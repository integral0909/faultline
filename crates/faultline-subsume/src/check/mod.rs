//! Deciding value-set inclusion between types of two APIs.
//!
//! [`Checker::subsumes`] answers whether every value of a *left* type is also
//! a value of a *right* type. The answer is three-valued:
//!
//! * [`Verdict::Yes`] is proven by rules that are sound for the constructs
//!   involved;
//! * [`Verdict::No`] carries a witness value that the validator has confirmed
//!   belongs to the left type and not to the right one;
//! * [`Verdict::Unknown`] is returned when neither could be established.
//!
//! Recursive types are compared coinductively: a pair of types met again
//! while it is still being compared is assumed to be included. A result that
//! relied on such an assumption about a pair *further up* the stack is not
//! cached until that pair is decided, which keeps the memo table sound.

mod scalar;
mod structure;

use std::collections::HashMap;

use faultline_ir::{Api, EnumType, Literal, TypeId, TypeKind, UnionMode, UnionType};

use crate::pattern;

use crate::sample::Sampler;
use crate::validate::{Membership, Validator};

/// The answer to an inclusion question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Yes,
    /// A value of the left type that is not a value of the right type.
    No(Literal),
    Unknown(String),
}

impl Verdict {
    pub fn is_yes(&self) -> bool {
        matches!(self, Verdict::Yes)
    }

    pub fn witness(&self) -> Option<&Literal> {
        match self {
            Verdict::No(w) => Some(w),
            _ => None,
        }
    }
}

/// Bounds the total work of one checker, so that pathological schemas
/// produce `Unknown` rather than unbounded run time.
const STEP_BUDGET: usize = 200_000;

/// Compares types of a left API against types of a right API.
pub struct Checker<'a> {
    pub(crate) left: &'a Api,
    pub(crate) right: &'a Api,
    pub(crate) left_values: Validator<'a>,
    pub(crate) right_values: Validator<'a>,
    pub(crate) left_samples: Sampler<'a>,
    pub(crate) right_samples: Sampler<'a>,
    memo: HashMap<(TypeId, TypeId), Verdict>,
    stack: Vec<(TypeId, TypeId)>,
    /// Lowest stack index an assumption was made about during the current
    /// computation (`usize::MAX` if none).
    lowest_assumption: usize,
    steps: usize,
}

impl std::fmt::Debug for Checker<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checker")
            .field("memoized", &self.memo.len())
            .field("steps", &self.steps)
            .finish_non_exhaustive()
    }
}

impl<'a> Checker<'a> {
    pub fn new(left: &'a Api, right: &'a Api) -> Self {
        Self {
            left,
            right,
            // The left side produces values and the right side accepts them.
            left_values: Validator::writer(left),
            right_values: Validator::new(right),
            left_samples: Sampler::writer(left),
            right_samples: Sampler::new(right),
            memo: HashMap::new(),
            stack: Vec::new(),
            lowest_assumption: usize::MAX,
            steps: 0,
        }
    }

    /// Whether every value of `left` (in the left API) is a value of `right`
    /// (in the right API).
    pub fn subsumes(&mut self, left: TypeId, right: TypeId) -> Verdict {
        let key = (
            self.left.types.resolve(left),
            self.right.types.resolve(right),
        );
        if let Some(verdict) = self.memo.get(&key) {
            return verdict.clone();
        }
        if let Some(position) = self.stack.iter().position(|pair| *pair == key) {
            self.lowest_assumption = self.lowest_assumption.min(position);
            return Verdict::Yes;
        }
        self.steps += 1;
        if self.steps > STEP_BUDGET {
            return Verdict::Unknown("comparison budget exhausted".into());
        }

        let depth = self.stack.len();
        self.stack.push(key);
        let outer = std::mem::replace(&mut self.lowest_assumption, usize::MAX);
        let verdict = self.compute(key.0, key.1);
        self.stack.pop();

        let depends_on_ancestor = self.lowest_assumption < depth;
        if !(verdict.is_yes() && depends_on_ancestor) {
            self.memo.insert(key, verdict.clone());
        }
        self.lowest_assumption = if depends_on_ancestor {
            outer.min(self.lowest_assumption)
        } else {
            outer
        };
        verdict
    }

    fn compute(&mut self, l: TypeId, r: TypeId) -> Verdict {
        let left: &'a Api = self.left;
        let right: &'a Api = self.right;
        let lk = &left.types.get(l).kind;
        let rk = &right.types.get(r).kind;
        match (lk, rk) {
            (_, TypeKind::Any)
            | (TypeKind::Never, _)
            | (TypeKind::Null, TypeKind::Null)
            | (TypeKind::Boolean, TypeKind::Boolean)
            | (TypeKind::Bytes, TypeKind::Bytes) => Verdict::Yes,
            (TypeKind::Enum(e), _) if !e.open || e.writer_exact => self.left_enum(e, r),
            (TypeKind::Union(u), _) => self.left_union(l, u, r),
            (_, TypeKind::AllOf(members)) => self.right_all_of(l, members, r),
            (TypeKind::AllOf(members), _) => self.left_all_of(l, members, r),
            (_, TypeKind::Union(u)) => self.right_union(l, u, r),
            (_, TypeKind::Not(operand)) => {
                if classes_disjoint(left, l, right, *operand) {
                    Verdict::Yes
                } else {
                    self.search(l, r, "a negated schema could not be decided")
                }
            }
            (
                TypeKind::Integer(a) | TypeKind::Number(a),
                TypeKind::Integer(b) | TypeKind::Number(b),
            ) => {
                let a_int = matches!(lk, TypeKind::Integer(_));
                let b_int = matches!(rk, TypeKind::Integer(_));
                if scalar::numeric_proven(a, a_int, b, b_int) {
                    Verdict::Yes
                } else {
                    self.search(l, r, "numeric constraints could not be compared exactly")
                }
            }
            (TypeKind::String(a), TypeKind::String(b)) => self.strings(l, a, r, b),
            (TypeKind::Array(a), TypeKind::Array(b)) => self.arrays(l, a, r, b),
            (TypeKind::Object(a), TypeKind::Object(b)) => self.objects(l, a, r, b),
            (TypeKind::Map(a), TypeKind::Map(b)) => {
                let keys = self.subsumes(a.key, b.key);
                let values = self.subsumes(a.value, b.value);
                if keys.is_yes() && values.is_yes() {
                    Verdict::Yes
                } else {
                    self.search(l, r, "map keys or values differ")
                }
            }
            _ if self.left_is_empty(l) => Verdict::Yes,
            _ => self.search(l, r, "the types have different shapes"),
        }
    }

    /// Whether the left type provably has no values, which makes it included
    /// in anything.
    fn left_is_empty(&self, l: TypeId) -> bool {
        match &self.left.types.get(l).kind {
            TypeKind::Never => true,
            TypeKind::Integer(n) => scalar::numeric_empty(n, true),
            TypeKind::Number(n) => scalar::numeric_empty(n, false),
            TypeKind::String(s) => scalar::string_empty(s),
            TypeKind::Array(a) => a.max_items.is_some_and(|max| max < a.min_items),
            TypeKind::Object(o) => o.max_properties.is_some_and(|max| max < o.min_properties),
            TypeKind::Enum(e) => (!e.open || e.writer_exact) && e.values.is_empty(),
            _ => false,
        }
    }

    /// An intersection is included if one member is. Otherwise a member's
    /// counterexample usually misses what the other members require (a
    /// composed object lacks the base's required fields), so it is merged
    /// with values of the other members and re-verified.
    fn left_all_of(&mut self, l: TypeId, members: &[TypeId], r: TypeId) -> Verdict {
        let mut witnesses = Vec::new();
        for &member in members {
            match self.subsumes(member, r) {
                Verdict::Yes => return Verdict::Yes,
                Verdict::No(w) => witnesses.push((member, w)),
                Verdict::Unknown(_) => {}
            }
        }
        for (member, witness) in witnesses {
            if self.confirmed(l, r, &witness) {
                return Verdict::No(witness);
            }
            for &other in members.iter().filter(|&&m| m != member) {
                for base in self.left_samples.candidates(other) {
                    if let Some(merged) = merge(&base, &witness)
                        && self.confirmed(l, r, &merged)
                    {
                        return Verdict::No(merged);
                    }
                }
            }
        }
        self.search(l, r, "an allOf could not be matched member by member")
    }

    /// Lengths are compared as bounds and patterns as regular languages: a
    /// left string within its lengths that the right pattern rejects is a
    /// witness once the whole types confirm it.
    fn strings(
        &self,
        l: TypeId,
        a: &faultline_ir::StringType,
        r: TypeId,
        b: &faultline_ir::StringType,
    ) -> Verdict {
        if scalar::string_empty(a) {
            return Verdict::Yes;
        }
        let lengths = scalar::lengths_proven(a, b);
        let patterns = match (&a.pattern, &b.pattern) {
            (_, None) => Some(None),
            (Some(x), Some(y)) if x == y => Some(None),
            (_, Some(_)) => {
                match (
                    pattern::language(a.pattern.as_ref()),
                    pattern::language(b.pattern.as_ref()),
                ) {
                    (Some(left), Some(right)) => {
                        let (min, max) = scalar::length_window(a);
                        Some(faultline_regex::difference(&left, &right, min, max))
                    }
                    _ => None,
                }
            }
        };
        match patterns {
            Some(None) if lengths => Verdict::Yes,
            Some(Some(witness)) => {
                let witness = Literal::string(witness);
                if self.confirmed(l, r, &witness) {
                    Verdict::No(witness)
                } else {
                    self.search(l, r, "string constraints could not be compared exactly")
                }
            }
            _ => self.search(l, r, "string constraints could not be compared exactly"),
        }
    }

    /// Every member of a closed enum must be a value of the right type.
    fn left_enum(&self, e: &EnumType, r: TypeId) -> Verdict {
        let mut unknown = None;
        for value in &e.values {
            match self.right_values.validate(r, &value.value) {
                Membership::Valid => {}
                Membership::Invalid(_) => return Verdict::No(value.value.clone()),
                Membership::Unknown(why) => {
                    unknown.get_or_insert(why);
                }
            }
        }
        unknown.map_or(Verdict::Yes, |why| Verdict::Unknown(why.into()))
    }

    /// A union is included if every variant is. For an exclusive union a
    /// variant's witness may match a second variant, and so not belong to the
    /// union; witnesses are therefore re-validated against the whole union.
    fn left_union(&mut self, l: TypeId, u: &UnionType, r: TypeId) -> Verdict {
        let mut unknown = None;
        for &variant in &u.variants {
            match self.subsumes(variant, r) {
                Verdict::Yes => {}
                Verdict::No(w) => {
                    if u.mode != UnionMode::Exclusive || self.left_values.validate(l, &w).is_valid()
                    {
                        return Verdict::No(w);
                    }
                    unknown.get_or_insert_with(|| {
                        "an exclusive union's witness matched several variants".to_owned()
                    });
                }
                Verdict::Unknown(why) => {
                    unknown.get_or_insert(why);
                }
            }
        }
        match unknown {
            None => Verdict::Yes,
            Some(why) => self.search_or_unknown(l, r, why),
        }
    }

    /// Inclusion in an intersection is inclusion in every member. A witness
    /// for one member is outside the intersection too.
    fn right_all_of(&mut self, l: TypeId, members: &[TypeId], r: TypeId) -> Verdict {
        let mut unknown = None;
        for &member in members {
            match self.subsumes(l, member) {
                Verdict::Yes => {}
                Verdict::No(w) => return Verdict::No(w),
                Verdict::Unknown(why) => {
                    unknown.get_or_insert(why);
                }
            }
        }
        match unknown {
            None => Verdict::Yes,
            Some(why) => self.search_or_unknown(l, r, why),
        }
    }

    fn right_union(&mut self, l: TypeId, u: &UnionType, r: TypeId) -> Verdict {
        let left: &'a Api = self.left;
        let right: &'a Api = self.right;
        // When the variants have pairwise different JSON types, the only
        // variant that can hold values of the left type decides the question
        // exactly (and exclusivity holds automatically).
        if let Some(left_class) = class_of(&left.types.get(l).kind)
            && let Some(classes) = u
                .variants
                .iter()
                .map(|&v| class_of(&right.types.get(right.types.resolve(v)).kind))
                .collect::<Option<Vec<_>>>()
            && all_distinct(&classes)
        {
            return match classes.iter().position(|c| *c == left_class) {
                Some(i) => match self.subsumes(l, u.variants[i]) {
                    Verdict::No(w) if self.right_values.validate(r, &w).is_invalid() => {
                        Verdict::No(w)
                    }
                    Verdict::No(_) => self.search(l, r, "union variants overlap"),
                    other => other,
                },
                None => self.search(l, r, "no union variant has the same JSON type"),
            };
        }
        for (i, &variant) in u.variants.iter().enumerate() {
            if self.subsumes(l, variant).is_yes()
                && (u.mode != UnionMode::Exclusive || self.disjoint_from_others(l, u, i))
            {
                return Verdict::Yes;
            }
        }
        // The left type may still be covered by several variants together;
        // only a verified counterexample settles it.
        self.search(
            l,
            r,
            "the left type is not contained in a single union variant",
        )
    }

    /// Whether the left type shares no value with any variant but `except`.
    fn disjoint_from_others(&self, l: TypeId, u: &UnionType, except: usize) -> bool {
        u.variants
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != except)
            .all(|(_, &v)| classes_disjoint(self.left, l, self.right, v))
    }

    /// Looks for a counterexample among values of the left type and probes
    /// derived from the right type.
    pub(crate) fn search(&self, l: TypeId, r: TypeId, why: &str) -> Verdict {
        self.search_or_unknown(l, r, why.to_owned())
    }

    fn search_or_unknown(&self, l: TypeId, r: TypeId, why: String) -> Verdict {
        match self.find_counterexample(l, r) {
            Some(w) => Verdict::No(w),
            None => Verdict::Unknown(why),
        }
    }

    pub(crate) fn find_counterexample(&self, l: TypeId, r: TypeId) -> Option<Literal> {
        let from_left = self.left_samples.candidates(l);
        let from_right = self
            .right_samples
            .probes(r)
            .into_iter()
            .filter(|v| self.left_values.validate(l, v).is_valid());
        from_left
            .into_iter()
            .chain(from_right)
            .find(|v| self.right_values.validate(r, v).is_invalid())
    }

    /// Confirms a witness: in the left type, not in the right one.
    pub(crate) fn confirmed(&self, l: TypeId, r: TypeId, witness: &Literal) -> bool {
        self.left_values.validate(l, witness).is_valid()
            && self.right_values.validate(r, witness).is_invalid()
    }
}

/// Two objects combined, with `overlay`'s members taking precedence.
fn merge(base: &Literal, overlay: &Literal) -> Option<Literal> {
    let (Literal::Object(base), Literal::Object(overlay)) = (base, overlay) else {
        return None;
    };
    let mut members = base.clone();
    members.extend(overlay.iter().map(|(k, v)| (k.clone(), v.clone())));
    Some(Literal::Object(members))
}

/// The JSON type of values a concrete kind holds; `None` for kinds whose
/// values may be of several JSON types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Null,
    Boolean,
    Number,
    String,
    Array,
    Object,
}

fn class_of(kind: &TypeKind) -> Option<Class> {
    Some(match kind {
        TypeKind::Null => Class::Null,
        TypeKind::Boolean => Class::Boolean,
        TypeKind::Integer(_) | TypeKind::Number(_) => Class::Number,
        TypeKind::String(_) | TypeKind::Bytes => Class::String,
        TypeKind::Array(_) => Class::Array,
        TypeKind::Object(_) | TypeKind::Map(_) => Class::Object,
        TypeKind::Enum(e) if !e.open => {
            let mut classes = e.values.iter().map(|v| literal_class(&v.value));
            let first = classes.next()?;
            return classes.all(|c| c == first).then_some(first);
        }
        _ => return None,
    })
}

fn literal_class(value: &Literal) -> Class {
    match value {
        Literal::Null => Class::Null,
        Literal::Bool(_) => Class::Boolean,
        Literal::Number(_) => Class::Number,
        Literal::String(_) => Class::String,
        Literal::Array(_) => Class::Array,
        Literal::Object(_) => Class::Object,
    }
}

fn all_distinct(classes: &[Class]) -> bool {
    classes
        .iter()
        .enumerate()
        .all(|(i, c)| !classes[..i].contains(c))
}

/// Whether two types provably share no value because their values have
/// different JSON types.
fn classes_disjoint(left: &Api, l: TypeId, right: &Api, r: TypeId) -> bool {
    let lk = &left.types.get(left.types.resolve(l)).kind;
    let rk = &right.types.get(right.types.resolve(r)).kind;
    matches!((class_of(lk), class_of(rk)), (Some(a), Some(b)) if a != b)
}
