//! Membership of a concrete value in an IR type.
//!
//! The validator is the ground truth every counterexample is checked against,
//! so it is exact wherever it answers `Valid` or `Invalid`, and says
//! `Unknown` where it cannot decide. Today that is regular expressions
//! (`pattern`, `patternProperties`), whose ECMA-262 semantics are handled by
//! a later layer, and arithmetic beyond exact `i128` range.
//!
//! `format` is treated as an annotation, as JSON Schema 2019-09 and later
//! specify by default: it never makes a value invalid.

use std::collections::BTreeMap;

use faultline_doc::Number;
use faultline_ir::{
    Api, ArrayType, Field, Literal, NumericType, ObjectType, StringType, TypeId, TypeKind,
    UnionMode,
};

use crate::rational::Q;

/// Why a value is not a member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The value's JSON type is not allowed (`expected` describes the type).
    WrongType {
        expected: &'static str,
    },
    BelowMinimum,
    AboveMaximum,
    NotMultiple,
    TooShort,
    TooLong,
    TooFewItems,
    TooManyItems,
    DuplicateItems,
    ContainsCount,
    TooFewProperties,
    TooManyProperties,
    /// Every required member the value lacks.
    MissingRequired(Vec<String>),
    UnexpectedProperty(String),
    InvalidPropertyName(String),
    DependentRequired {
        property: String,
        missing: String,
    },
    /// Two members of an exclusive group are both present.
    ExclusiveMembers {
        group: String,
        first: String,
        second: String,
    },
    /// A string does not contain a match of the pattern.
    PatternMismatch,
    NotInEnum,
    NoVariantMatches,
    SeveralVariantsMatch,
    NegatedMatch,
    Never,
}

/// The result of a membership test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Membership {
    Valid,
    /// Invalid, with the reason found at the innermost failing position.
    Invalid(Reason),
    Unknown(&'static str),
}

impl Membership {
    pub fn is_valid(&self) -> bool {
        matches!(self, Membership::Valid)
    }

    pub fn is_invalid(&self) -> bool {
        matches!(self, Membership::Invalid(_))
    }
}

/// Accumulates the results of several independent checks that must all pass:
/// the first definite failure wins; otherwise any unknown makes the whole
/// unknown.
#[derive(Default)]
struct All {
    unknown: Option<&'static str>,
}

impl All {
    /// Returns `Some` if `result` settles the conjunction as invalid.
    fn add(&mut self, result: Membership) -> Option<Membership> {
        match result {
            Membership::Valid => None,
            Membership::Unknown(why) => {
                self.unknown.get_or_insert(why);
                None
            }
            invalid @ Membership::Invalid(_) => Some(invalid),
        }
    }

    fn finish(self) -> Membership {
        self.unknown.map_or(Membership::Valid, Membership::Unknown)
    }
}

const MAX_DEPTH: usize = 256;

/// Validates values against the types of one [`Api`].
///
/// Types describe what readers accept. Some formats write less than they
/// read (see [`ObjectType::writer_exact`]); a validator for *written* values
/// holds those types to what writers produce.
#[derive(Clone, Copy, Debug)]
pub struct Validator<'a> {
    api: &'a Api,
    written: bool,
}

impl<'a> Validator<'a> {
    /// A validator for values as read.
    pub fn new(api: &'a Api) -> Self {
        Self {
            api,
            written: false,
        }
    }

    /// A validator for values as written.
    pub fn writer(api: &'a Api) -> Self {
        Self { api, written: true }
    }

    pub fn api(self) -> &'a Api {
        self.api
    }

    pub fn validate(self, ty: TypeId, value: &Literal) -> Membership {
        self.check(ty, value, 0)
    }

    fn check(self, ty: TypeId, value: &Literal, depth: usize) -> Membership {
        if depth > MAX_DEPTH {
            return Membership::Unknown("value nesting exceeds the validation depth limit");
        }
        let node = self.api.types.get(ty);
        match &node.kind {
            TypeKind::Any => Membership::Valid,
            TypeKind::Never => Membership::Invalid(Reason::Never),
            TypeKind::Null => expect(matches!(value, Literal::Null), "null"),
            TypeKind::Boolean => expect(matches!(value, Literal::Bool(_)), "boolean"),
            TypeKind::Bytes => expect(matches!(value, Literal::String(_)), "bytes"),
            TypeKind::Integer(n) => check_number(n, value, true),
            TypeKind::Number(n) => check_number(n, value, false),
            TypeKind::String(s) => check_string(s, value),
            TypeKind::Enum(e) => {
                let open = e.open && !(self.written && e.writer_exact);
                if e.values.iter().any(|v| v.value == *value) {
                    Membership::Valid
                } else if open && e.values.iter().any(|v| same_json_type(&v.value, value)) {
                    // Open enums document known values; others of the same
                    // type are allowed.
                    Membership::Valid
                } else {
                    Membership::Invalid(Reason::NotInEnum)
                }
            }
            TypeKind::Object(o) => self.check_object(o, value, depth),
            TypeKind::Array(a) => self.check_array(a, value, depth),
            TypeKind::Map(m) => {
                let Literal::Object(members) = value else {
                    return Membership::Invalid(Reason::WrongType { expected: "object" });
                };
                let mut all = All::default();
                for (key, member) in members {
                    if let Some(bad) =
                        all.add(self.check(m.key, &Literal::String(key.clone()), depth + 1))
                    {
                        return bad;
                    }
                    if let Some(bad) = all.add(self.check(m.value, member, depth + 1)) {
                        return bad;
                    }
                }
                all.finish()
            }
            TypeKind::Union(u) => self.check_union(u.mode, &u.variants, value, depth),
            TypeKind::AllOf(members) => {
                let mut all = All::default();
                for &member in members {
                    if let Some(bad) = all.add(self.check(member, value, depth + 1)) {
                        return bad;
                    }
                }
                all.finish()
            }
            TypeKind::Not(operand) => match self.check(*operand, value, depth + 1) {
                Membership::Valid => Membership::Invalid(Reason::NegatedMatch),
                Membership::Invalid(_) => Membership::Valid,
                unknown @ Membership::Unknown(_) => unknown,
            },
            TypeKind::Ref(target) => self.check(*target, value, depth + 1),
        }
    }

    fn check_union(
        self,
        mode: UnionMode,
        variants: &[TypeId],
        value: &Literal,
        depth: usize,
    ) -> Membership {
        let mut valid = 0usize;
        let mut unknown = None;
        for &variant in variants {
            match self.check(variant, value, depth + 1) {
                Membership::Valid => {
                    valid += 1;
                    if mode != UnionMode::Exclusive {
                        return Membership::Valid;
                    }
                }
                Membership::Invalid(_) => {}
                Membership::Unknown(why) => {
                    unknown.get_or_insert(why);
                }
            }
        }
        match (mode, valid, unknown) {
            (UnionMode::Exclusive, 1, None) => Membership::Valid,
            (UnionMode::Exclusive, n, _) if n >= 2 => {
                Membership::Invalid(Reason::SeveralVariantsMatch)
            }
            (_, 0, None) => Membership::Invalid(Reason::NoVariantMatches),
            (_, _, Some(why)) => Membership::Unknown(why),
            (_, _, None) => Membership::Valid,
        }
    }

    fn check_object(self, o: &ObjectType, value: &Literal, depth: usize) -> Membership {
        let Literal::Object(members) = value else {
            return Membership::Invalid(Reason::WrongType { expected: "object" });
        };
        let exact = self.written && o.writer_exact;
        // Readers also accept a field under its aliases; writers use its
        // name.
        let read = !self.written;
        let present = |f: &Field| {
            members.contains_key(&f.name)
                || (read && f.aliases.iter().any(|a| members.contains_key(a)))
        };
        let missing: Vec<String> = o
            .fields
            .iter()
            .filter(|f| (f.required || exact) && !present(f))
            .map(|f| f.name.clone())
            .collect();
        if !missing.is_empty() {
            return Membership::Invalid(Reason::MissingRequired(missing));
        }
        if exact && let Some(extra) = members.keys().find(|k| o.field(k).is_none()) {
            return Membership::Invalid(Reason::UnexpectedProperty(extra.clone()));
        }
        let count = members.len() as u64;
        if count < o.min_properties {
            return Membership::Invalid(Reason::TooFewProperties);
        }
        if o.max_properties.is_some_and(|max| count > max) {
            return Membership::Invalid(Reason::TooManyProperties);
        }
        for (property, required) in &o.dependent_required {
            if members.contains_key(property)
                && let Some(missing) = required.iter().find(|r| !members.contains_key(*r))
            {
                return Membership::Invalid(Reason::DependentRequired {
                    property: property.clone(),
                    missing: missing.clone(),
                });
            }
        }
        for group in &o.exclusive {
            let mut present = group.fields.iter().filter(|f| members.contains_key(*f));
            if let (Some(first), Some(second)) = (present.next(), present.next()) {
                return Membership::Invalid(Reason::ExclusiveMembers {
                    group: group.name.clone(),
                    first: first.clone(),
                    second: second.clone(),
                });
            }
        }
        let mut all = All::default();
        for (key, member) in members {
            if let Some(names) = o.property_names {
                match self.check(names, &Literal::String(key.clone()), depth + 1) {
                    Membership::Invalid(_) => {
                        return Membership::Invalid(Reason::InvalidPropertyName(key.clone()));
                    }
                    other => {
                        all.add(other);
                    }
                }
            }
            let field = o.field(key).or_else(|| {
                read.then(|| o.fields.iter().find(|f| f.aliases.iter().any(|a| a == key)))
                    .flatten()
            });
            let result = if let Some(field) = field {
                self.check(field.ty, member, depth + 1)
            } else {
                self.check_undeclared(o, key, member, depth)
            };
            if let Some(bad) = all.add(result) {
                return bad;
            }
        }
        all.finish()
    }

    /// A member no field declares: it must satisfy every pattern property
    /// whose pattern its name matches, and `additional` if none does.
    fn check_undeclared(
        self,
        o: &ObjectType,
        key: &str,
        member: &Literal,
        depth: usize,
    ) -> Membership {
        let mut all = All::default();
        let mut matched = false;
        for (pattern, ty) in &o.pattern_properties {
            let Some(regex) = crate::pattern::compiled(pattern) else {
                return Membership::Unknown("a patternProperties pattern cannot be analysed");
            };
            if regex.is_match(key) {
                matched = true;
                if let Some(bad) = all.add(self.check(*ty, member, depth + 1)) {
                    return bad;
                }
            }
        }
        if matched {
            return all.finish();
        }
        match self.check(o.additional, member, depth + 1) {
            Membership::Invalid(Reason::Never) => {
                Membership::Invalid(Reason::UnexpectedProperty(key.to_owned()))
            }
            other => other,
        }
    }

    fn check_array(self, a: &ArrayType, value: &Literal, depth: usize) -> Membership {
        let Literal::Array(items) = value else {
            return Membership::Invalid(Reason::WrongType { expected: "array" });
        };
        let count = items.len() as u64;
        if count < a.min_items {
            return Membership::Invalid(Reason::TooFewItems);
        }
        if a.max_items.is_some_and(|max| count > max) {
            return Membership::Invalid(Reason::TooManyItems);
        }
        if a.unique_items {
            for (i, item) in items.iter().enumerate() {
                if items[..i].contains(item) {
                    return Membership::Invalid(Reason::DuplicateItems);
                }
            }
        }
        let mut all = All::default();
        for (i, item) in items.iter().enumerate() {
            let ty = a.prefix_items.get(i).copied().unwrap_or(a.items);
            if let Some(bad) = all.add(self.check(ty, item, depth + 1)) {
                return bad;
            }
        }
        if let Some(contains) = &a.contains {
            let (mut matched, mut undecided) = (0u64, 0u64);
            for item in items {
                match self.check(contains.ty, item, depth + 1) {
                    Membership::Valid => matched += 1,
                    Membership::Invalid(_) => {}
                    Membership::Unknown(_) => undecided += 1,
                }
            }
            let max = contains.max.unwrap_or(u64::MAX);
            if matched + undecided < contains.min || matched > max {
                return Membership::Invalid(Reason::ContainsCount);
            }
            if undecided > 0 && (matched < contains.min || matched + undecided > max) {
                return Membership::Unknown("contains depends on an undecidable item");
            }
        }
        all.finish()
    }
}

fn expect(ok: bool, expected: &'static str) -> Membership {
    if ok {
        Membership::Valid
    } else {
        Membership::Invalid(Reason::WrongType { expected })
    }
}

fn same_json_type(a: &Literal, b: &Literal) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

fn check_number(n: &NumericType, value: &Literal, integer: bool) -> Membership {
    let Literal::Number(Number::Finite(d)) = value else {
        let expected = if integer { "integer" } else { "number" };
        return Membership::Invalid(Reason::WrongType { expected });
    };
    if integer && !d.is_integer() {
        return Membership::Invalid(Reason::WrongType {
            expected: "integer",
        });
    }
    if let Some(min) = &n.minimum {
        let below = if min.exclusive {
            *d <= min.value
        } else {
            *d < min.value
        };
        if below {
            return Membership::Invalid(Reason::BelowMinimum);
        }
    }
    if let Some(max) = &n.maximum {
        let above = if max.exclusive {
            *d >= max.value
        } else {
            *d > max.value
        };
        if above {
            return Membership::Invalid(Reason::AboveMaximum);
        }
    }
    if let Some(step) = &n.multiple_of {
        let multiple = Q::from_decimal(d)
            .zip(Q::from_decimal(step))
            .and_then(|(v, s)| v.is_multiple_of(s));
        match multiple {
            Some(true) => {}
            Some(false) => return Membership::Invalid(Reason::NotMultiple),
            None => return Membership::Unknown("multipleOf exceeds exact arithmetic range"),
        }
    }
    Membership::Valid
}

fn check_string(s: &StringType, value: &Literal) -> Membership {
    let Literal::String(text) = value else {
        return Membership::Invalid(Reason::WrongType { expected: "string" });
    };
    let length = text.chars().count() as u64;
    if length < s.min_length {
        return Membership::Invalid(Reason::TooShort);
    }
    if s.max_length.is_some_and(|max| length > max) {
        return Membership::Invalid(Reason::TooLong);
    }
    if let Some(pattern) = &s.pattern {
        return match crate::pattern::compiled(pattern) {
            Some(regex) if regex.is_match(text) => Membership::Valid,
            Some(_) => Membership::Invalid(Reason::PatternMismatch),
            None => Membership::Unknown("the pattern uses constructs that cannot be analysed"),
        };
    }
    Membership::Valid
}

/// Convenience for building object literals in tests and samplers.
pub(crate) fn object(members: impl IntoIterator<Item = (String, Literal)>) -> Literal {
    Literal::Object(members.into_iter().collect::<BTreeMap<_, _>>())
}
