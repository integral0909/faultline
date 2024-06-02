//! Classification of changes into findings.
//!
//! Whether a change breaks consumers depends on who is affected. Every
//! position has a polarity: at an *in* position the client sends values and
//! the API receives them; at an *out* position the API sends and the client
//! receives (callbacks and webhooks swap the two). A change breaks when it
//! hurts the client:
//!
//! * when the client sends, anything that makes previously valid messages
//!   invalid breaks it — a new required parameter, a tighter bound;
//! * when the client receives, anything that makes messages it may get
//!   unexpected breaks it — a new union alternative, a removed guarantee.
//!
//! Surface changes carry a pair of severities, one for each perspective.
//! Value-level changes derive theirs from the evidence the diff attached:
//! values *lost* matter to senders, values *gained* to receivers, and a
//! verified witness makes a break certain while an undecided question makes
//! it risky.

use std::fmt;

use faultline_diff::{Change, ChangeKind, Effect, Evidence};
use faultline_ir::Polarity;

use crate::catalog::{self, Rule};

/// How much a finding matters. Ordered from least to most severe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Compatible; reported for information.
    Info,
    /// May break some consumers, or could not be decided.
    Risky,
    /// Breaks consumers.
    Breaking,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Risky => "risky",
            Severity::Breaking => "breaking",
        }
    }

    pub fn parse(text: &str) -> Option<Severity> {
        match text {
            "info" => Some(Severity::Info),
            "risky" => Some(Severity::Risky),
            "breaking" => Some(Severity::Breaking),
            _ => None,
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A classified change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub rule: &'static Rule,
    pub severity: Severity,
    pub message: String,
    /// Stable identifier used by waivers and baselines; see [`finding_id`].
    pub id: String,
    /// The API the change belongs to.
    pub api: String,
    pub change: Change,
}

impl Finding {
    /// A verified value the new version rejects (at an *in* position) or
    /// newly produces (at an *out* position), whichever explains the
    /// severity.
    pub fn witness(&self) -> Option<&faultline_ir::Literal> {
        let effect = self.change.effect.as_ref()?;
        let polarity = self.change.polarity;
        if polarity.is_in()
            && let Some(w) = effect.lost.witness()
        {
            return Some(w);
        }
        if polarity.is_out() {
            return effect.gained.witness();
        }
        None
    }
}

/// Severity when the client sends and when it receives.
#[derive(Clone, Copy)]
struct Pair {
    sends: Severity,
    receives: Severity,
}

const fn pair(sends: Severity, receives: Severity) -> Pair {
    Pair { sends, receives }
}

const fn same(severity: Severity) -> Pair {
    pair(severity, severity)
}

use Severity::{Breaking, Info, Risky};

/// Classifies `change`, which belongs to the API named `api`.
pub fn classify(api: &str, change: &Change) -> Finding {
    let (rule, message, detail) = describe(&change.kind);
    let severity = severity(change);
    Finding {
        rule,
        severity,
        id: finding_id(api, rule.code, &change.path.to_string(), &detail),
        message,
        api: api.to_owned(),
        change: change.clone(),
    }
}

fn severity(change: &Change) -> Severity {
    let polarity = change.polarity;
    if polarity.is_none() {
        return Info;
    }
    match surface_pair(&change.kind) {
        Some(Surface::Request(p) | Surface::Value(p)) => by_sender(p, polarity),
        Some(Surface::Response(p)) => by_receiver(p, polarity),
        None => value_severity(change, polarity),
    }
}

/// Request-side changes: the client sends at *in* positions.
fn by_sender(p: Pair, polarity: Polarity) -> Severity {
    let mut s = Info;
    if polarity.is_in() {
        s = s.max(p.sends);
    }
    if polarity.is_out() {
        s = s.max(p.receives);
    }
    s
}

/// Response-side changes: the client receives at *out* positions.
fn by_receiver(p: Pair, polarity: Polarity) -> Severity {
    let mut s = Info;
    if polarity.is_out() {
        s = s.max(p.receives);
    }
    if polarity.is_in() {
        s = s.max(p.sends);
    }
    s
}

enum Surface {
    Request(Pair),
    Response(Pair),
    /// A type-level change whose severity does not depend on evidence.
    Value(Pair),
}

/// Fixed severities for changes that do not carry value-level evidence.
///
/// One arm per change kind, like a table: merging arms that happen to share
/// a severity would hide which kinds were decided deliberately.
#[allow(clippy::match_same_arms)]
fn surface_pair(kind: &ChangeKind) -> Option<Surface> {
    use ChangeKind as K;
    use Surface::{Request, Response, Value};
    Some(match kind {
        K::OperationRemoved => Request(pair(Breaking, Risky)),
        K::OperationAdded => Request(pair(Info, Risky)),
        K::OperationIdChanged { .. } => Request(same(Risky)),
        K::ParameterAdded { required: true, .. } | K::RequestBodyAdded { required: true } => {
            Request(pair(Breaking, Info))
        }
        K::ParameterAdded {
            required: false, ..
        }
        | K::RequestBodyAdded { required: false } => Request(same(Info)),
        K::ParameterRemoved { required, .. } => {
            Request(pair(Risky, if *required { Breaking } else { Risky }))
        }
        K::ParameterRequiredChanged { required: true, .. }
        | K::RequestBodyRequiredChanged { required: true } => Request(pair(Breaking, Info)),
        K::ParameterRequiredChanged {
            required: false, ..
        }
        | K::RequestBodyRequiredChanged { required: false } => Request(pair(Info, Breaking)),
        K::ParameterSerializationChanged { .. } => Request(same(Breaking)),
        K::RequestBodyRemoved => Request(pair(Risky, Breaking)),
        K::MediaTypeAdded { .. } => Request(pair(Info, Risky)),
        K::MediaTypeRemoved { .. } => Request(same(Breaking)),
        K::SecurityRequirementAdded { .. } => Request(same(Info)),
        K::SecurityRequirementRemoved { .. } => Request(pair(Breaking, Info)),
        K::CallbackAdded { .. } => Request(same(Info)),
        K::CallbackRemoved { .. } => Request(same(Risky)),
        K::ResponseAdded { .. } => Response(pair(Info, Risky)),
        K::ResponseRemoved { .. } => Response(pair(Risky, Info)),
        K::ResponseHeaderAdded { required, .. } => {
            Response(pair(if *required { Breaking } else { Info }, Info))
        }
        K::ResponseHeaderRemoved { required, .. } => {
            Response(pair(Info, if *required { Breaking } else { Risky }))
        }
        K::ResponseHeaderRequiredChanged { required: true, .. } => Response(pair(Breaking, Info)),
        K::ResponseHeaderRequiredChanged {
            required: false, ..
        } => Response(pair(Info, Breaking)),
        K::WebhookAdded { .. } => Response(same(Info)),
        K::WebhookRemoved { .. } => Response(same(Risky)),
        K::SchemaAdded { .. } | K::DeprecatedChanged { .. } => Value(same(Info)),
        K::SchemaRemoved { .. }
        | K::DiscriminatorChanged { .. }
        | K::DiscriminatorValueRetargeted { .. } => Value(same(Breaking)),
        K::DiscriminatorValueAdded { .. } => Value(pair(Info, Risky)),
        K::DiscriminatorValueRemoved { .. } => Value(pair(Breaking, Info)),
        K::AccessChanged { .. } | K::DefaultChanged { .. } => Value(same(Risky)),
        K::FieldRenamed { .. } | K::EnumValueRenamed { .. } => Value(same(Risky)),
        K::FieldTagChanged { .. }
        | K::EnumValueRenumbered { .. }
        | K::EncodingChanged { .. }
        | K::StreamingChanged { .. }
        | K::RetiredNumberReused { .. } => Value(same(Breaking)),
        K::NumberNotRetired { .. } | K::RetirementRemoved { .. } => Value(same(Risky)),
        K::FieldAliasesChanged { removed, .. } => {
            Value(same(if removed.is_empty() { Info } else { Risky }))
        }
        K::TypeChanged { .. }
        | K::FieldAdded { .. }
        | K::FieldRemoved { .. }
        | K::FieldRequiredChanged { .. }
        | K::FieldGroupChanged { .. }
        | K::EnumValueAdded { .. }
        | K::EnumValueRemoved { .. }
        | K::BoundChanged { .. }
        | K::NumericKindChanged { .. }
        | K::PatternChanged { .. }
        | K::FormatChanged { .. }
        | K::UniqueItemsChanged { .. }
        | K::VariantAdded { .. }
        | K::VariantRemoved { .. }
        | K::UnionModeChanged { .. }
        | K::ConstraintChanged => return None,
    })
}

/// Severity of a value-level change from its evidence.
fn value_severity(change: &Change, polarity: Polarity) -> Severity {
    let Some(effect) = &change.effect else {
        // Value-level kinds reported without evidence (composition changes
        // the walk could not align) cannot be decided.
        return Risky;
    };
    let kind = &change.kind;
    // Optional request properties are, by near-universal convention, a
    // compatible addition, even though a client that happened to send the
    // name with another type is now rejected.
    if matches!(
        kind,
        ChangeKind::FieldAdded {
            required: false,
            ..
        }
    ) && !polarity.is_out()
    {
        return Info;
    }
    let mut s = Info;
    if polarity.is_in() {
        s = s.max(evidence(&effect.lost, Breaking));
    }
    if polarity.is_out() {
        s = s.max(evidence(&effect.gained, receiver_gain(kind)));
    }
    if matches!(kind, ChangeKind::FormatChanged { .. }) && s == Info && !is_trivial(effect) {
        s = Risky;
    }
    s
}

/// Severity for one direction of evidence: certain with a witness,
/// risky when undecided.
fn evidence(e: &Evidence, certain: Severity) -> Severity {
    match e {
        Evidence::None => Info,
        Evidence::Witness(_) => certain,
        Evidence::Unknown => certain.min(Risky),
    }
}

/// How bad it is for a receiver to get values it could not get before.
fn receiver_gain(kind: &ChangeKind) -> Severity {
    use ChangeKind as K;
    match kind {
        K::TypeChanged { .. }
        | K::FieldRequiredChanged { .. }
        | K::FieldRemoved { required: true, .. }
        | K::VariantAdded { .. }
        | K::NumericKindChanged { .. }
        | K::UniqueItemsChanged { .. }
        | K::ConstraintChanged => Breaking,
        _ => Risky,
    }
}

fn is_trivial(effect: &Effect) -> bool {
    effect.lost.is_none() && effect.gained.is_none()
}

/// The rule, message and identity detail for a change kind.
///
/// A single exhaustive match, so that adding a change kind without a rule
/// fails to compile; splitting it would weaken that guarantee.
#[allow(clippy::too_many_lines)]
fn describe(kind: &ChangeKind) -> (&'static Rule, String, String) {
    use ChangeKind as K;
    use catalog as c;
    let none = || String::new();
    let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "none".to_owned());
    match kind {
        K::OperationAdded => (&c::OPERATION_ADDED, "operation added".into(), none()),
        K::OperationRemoved => (&c::OPERATION_REMOVED, "operation removed".into(), none()),
        K::OperationIdChanged { old, new } => (
            &c::OPERATION_ID_CHANGED,
            format!("operationId changed from {} to {}", show(old), show(new)),
            none(),
        ),
        K::ParameterAdded {
            location,
            name,
            required,
        } => (
            &c::PARAMETER_ADDED,
            format!(
                "{}{location} parameter `{name}` added",
                if *required { "required " } else { "" }
            ),
            none(),
        ),
        K::ParameterRemoved { location, name, .. } => (
            &c::PARAMETER_REMOVED,
            format!("{location} parameter `{name}` removed"),
            none(),
        ),
        K::ParameterRequiredChanged {
            location,
            name,
            required,
        } => (
            &c::PARAMETER_REQUIREMENT_CHANGED,
            format!(
                "{location} parameter `{name}` is now {}",
                if *required { "required" } else { "optional" }
            ),
            none(),
        ),
        K::ParameterSerializationChanged {
            location,
            name,
            old,
            new,
        } => (
            &c::PARAMETER_SERIALIZATION_CHANGED,
            format!("serialization of {location} parameter `{name}` changed from {old} to {new}"),
            none(),
        ),
        K::RequestBodyAdded { required } => (
            &c::REQUEST_BODY_ADDED,
            format!(
                "{}request body added",
                if *required { "required " } else { "" }
            ),
            none(),
        ),
        K::RequestBodyRemoved => (
            &c::REQUEST_BODY_REMOVED,
            "request body removed".into(),
            none(),
        ),
        K::RequestBodyRequiredChanged { required } => (
            &c::REQUEST_BODY_REQUIREMENT_CHANGED,
            format!(
                "request body is now {}",
                if *required { "required" } else { "optional" }
            ),
            none(),
        ),
        K::MediaTypeAdded { media_type } => (
            &c::MEDIA_TYPE_ADDED,
            format!("media type {media_type} added"),
            none(),
        ),
        K::MediaTypeRemoved { media_type } => (
            &c::MEDIA_TYPE_REMOVED,
            format!("media type {media_type} removed"),
            none(),
        ),
        K::ResponseAdded { status } => (
            &c::RESPONSE_ADDED,
            format!("response {status} added"),
            none(),
        ),
        K::ResponseRemoved { status } => (
            &c::RESPONSE_REMOVED,
            format!("response {status} removed"),
            none(),
        ),
        K::ResponseHeaderAdded { name, required } => (
            &c::RESPONSE_HEADER_ADDED,
            format!(
                "{}response header `{name}` added",
                if *required { "required " } else { "" }
            ),
            none(),
        ),
        K::ResponseHeaderRemoved { name, .. } => (
            &c::RESPONSE_HEADER_REMOVED,
            format!("response header `{name}` removed"),
            none(),
        ),
        K::ResponseHeaderRequiredChanged { name, required } => (
            &c::RESPONSE_HEADER_REQUIREMENT_CHANGED,
            format!(
                "response header `{name}` is now {}",
                if *required { "required" } else { "optional" }
            ),
            none(),
        ),
        K::SecurityRequirementAdded { requirement } => (
            &c::SECURITY_REQUIREMENT_ADDED,
            format!("security alternative {} added", security_name(requirement)),
            requirement.clone(),
        ),
        K::SecurityRequirementRemoved { requirement } => (
            &c::SECURITY_REQUIREMENT_REMOVED,
            if requirement == "{}" {
                "anonymous access removed: the operation now requires authentication".to_owned()
            } else {
                format!(
                    "security alternative {} removed",
                    security_name(requirement)
                )
            },
            requirement.clone(),
        ),
        K::CallbackAdded { name } => (
            &c::CALLBACK_ADDED,
            format!("callback `{name}` added"),
            none(),
        ),
        K::CallbackRemoved { name } => (
            &c::CALLBACK_REMOVED,
            format!("callback `{name}` removed"),
            none(),
        ),
        K::WebhookAdded { name } => (&c::WEBHOOK_ADDED, format!("webhook `{name}` added"), none()),
        K::WebhookRemoved { name } => (
            &c::WEBHOOK_REMOVED,
            format!("webhook `{name}` removed"),
            none(),
        ),
        K::SchemaAdded { name } => (&c::SCHEMA_ADDED, format!("schema `{name}` added"), none()),
        K::SchemaRemoved { name } => (
            &c::SCHEMA_REMOVED,
            format!("schema `{name}` removed"),
            none(),
        ),
        K::TypeChanged { old, new } => (
            &c::TYPE_CHANGED,
            format!("type changed from {old} to {new}"),
            none(),
        ),
        K::FieldAdded { name, required } => (
            &c::PROPERTY_ADDED,
            format!(
                "{}property `{name}` added",
                if *required { "required " } else { "" }
            ),
            name.clone(),
        ),
        K::FieldRemoved { name, .. } => (
            &c::PROPERTY_REMOVED,
            format!("property `{name}` removed"),
            name.clone(),
        ),
        K::FieldRequiredChanged { name, required } => (
            &c::PROPERTY_REQUIREMENT_CHANGED,
            format!(
                "property `{name}` is now {}",
                if *required { "required" } else { "optional" }
            ),
            name.clone(),
        ),
        K::EnumValueAdded { value } => (
            &c::ENUM_VALUE_ADDED,
            format!("enum value {value} added"),
            value.to_string(),
        ),
        K::EnumValueRemoved { value } => (
            &c::ENUM_VALUE_REMOVED,
            format!("enum value {value} removed"),
            value.to_string(),
        ),
        K::EnumValueRenumbered { name, old, new } => (
            &c::ENUM_VALUE_RENUMBERED,
            format!("enum value {name} renumbered from {old} to {new}"),
            name.clone(),
        ),
        K::EnumValueRenamed { number, old, new } => (
            &c::ENUM_VALUE_RENAMED,
            format!("enum value {number} renamed from {old} to {new}"),
            number.to_string(),
        ),
        K::FieldRenamed { old, new, tag } => (
            &c::FIELD_RENAMED,
            match tag {
                Some(tag) => format!("field {tag} renamed from `{old}` to `{new}`"),
                None => format!("field `{old}` renamed to `{new}`, matched through an alias"),
            },
            none(),
        ),
        K::FieldTagChanged { name, old, new } => (
            &c::FIELD_NUMBER_CHANGED,
            format!("field `{name}` moved from number {old} to {new}"),
            none(),
        ),
        K::FieldAliasesChanged {
            name,
            removed,
            added,
        } => {
            let list = |names: &[String]| {
                names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let message = match (removed.is_empty(), added.is_empty()) {
                (false, true) => {
                    format!("field `{name}` is no longer accepted as {}", list(removed))
                }
                (true, false) => format!("field `{name}` is now also accepted as {}", list(added)),
                _ => format!(
                    "field `{name}` is no longer accepted as {} but as {}",
                    list(removed),
                    list(added)
                ),
            };
            (&c::FIELD_ALIASES_CHANGED, message, none())
        }
        K::FieldGroupChanged { name, old, new } => {
            let message = match (old, new) {
                (None, Some(g)) => format!("field `{name}` moved into oneof `{g}`"),
                (Some(g), None) => format!("field `{name}` moved out of oneof `{g}`"),
                (Some(a), Some(b)) if a != b => {
                    format!("field `{name}` moved from oneof `{a}` to `{b}`")
                }
                (Some(g), Some(_)) => format!("the members of oneof `{g}` around `{name}` changed"),
                (None, None) => format!("the grouping of field `{name}` changed"),
            };
            (&c::FIELD_GROUP_CHANGED, message, name.clone())
        }
        K::NumberNotRetired { name, number } => (
            &c::NUMBER_NOT_RESERVED,
            format!("`{name}` was removed without reserving number {number}"),
            name.clone(),
        ),
        K::RetiredNumberReused { name, number } => (
            &c::RESERVED_NUMBER_REUSED,
            format!("`{name}` reuses the reserved number {number}"),
            name.clone(),
        ),
        K::RetirementRemoved { what } => (
            &c::RESERVATION_REMOVED,
            format!("{what} no longer reserved"),
            what.clone(),
        ),
        K::EncodingChanged { old, new } => (
            &c::ENCODING_CHANGED,
            format!("wire encoding changed from {} to {}", show(old), show(new)),
            none(),
        ),
        K::StreamingChanged { side, streaming } => (
            &c::STREAMING_CHANGED,
            format!(
                "the {side} is {}",
                if *streaming {
                    "now a stream"
                } else {
                    "no longer a stream"
                }
            ),
            side.clone(),
        ),
        K::BoundChanged { facet, old, new } => (
            &c::BOUND_CHANGED,
            format!(
                "{} changed from {} to {}",
                facet.keyword(),
                show(old),
                show(new)
            ),
            facet.keyword().to_owned(),
        ),
        K::NumericKindChanged { integer } => (
            &c::NUMERIC_KIND_CHANGED,
            if *integer {
                "number became integer"
            } else {
                "integer became number"
            }
            .to_owned(),
            none(),
        ),
        K::PatternChanged { old, new } => (
            &c::PATTERN_CHANGED,
            format!("pattern changed from {} to {}", show(old), show(new)),
            none(),
        ),
        K::FormatChanged { old, new } => (
            &c::FORMAT_CHANGED,
            format!("format changed from {} to {}", show(old), show(new)),
            none(),
        ),
        K::UniqueItemsChanged { unique } => (
            &c::UNIQUENESS_CHANGED,
            if *unique {
                "items must now be unique"
            } else {
                "items no longer need to be unique"
            }
            .to_owned(),
            none(),
        ),
        K::VariantAdded { index } => (
            &c::VARIANT_ADDED,
            "union alternative added".into(),
            index.to_string(),
        ),
        K::VariantRemoved { index } => (
            &c::VARIANT_REMOVED,
            "union alternative removed".into(),
            index.to_string(),
        ),
        K::UnionModeChanged { old, new } => (
            &c::UNION_MODE_CHANGED,
            format!("union changed from {old} to {new}"),
            none(),
        ),
        K::ConstraintChanged => (
            &c::COMPOSITION_CHANGED,
            "composed constraints changed".into(),
            none(),
        ),
        K::DiscriminatorChanged { old, new } => (
            &c::DISCRIMINATOR_CHANGED,
            format!("discriminator changed from {} to {}", show(old), show(new)),
            none(),
        ),
        K::DiscriminatorValueAdded { value } => (
            &c::DISCRIMINATOR_VALUE_ADDED,
            format!("discriminator value `{value}` added"),
            value.clone(),
        ),
        K::DiscriminatorValueRemoved { value } => (
            &c::DISCRIMINATOR_VALUE_REMOVED,
            format!("discriminator value `{value}` removed"),
            value.clone(),
        ),
        K::DiscriminatorValueRetargeted { value, old, new } => (
            &c::DISCRIMINATOR_VALUE_RETARGETED,
            format!("discriminator value `{value}` now selects {new} instead of {old}"),
            value.clone(),
        ),
        K::DeprecatedChanged { deprecated } => (
            &c::DEPRECATION_CHANGED,
            if *deprecated {
                "deprecated"
            } else {
                "no longer deprecated"
            }
            .to_owned(),
            none(),
        ),
        K::AccessChanged { old, new } => (
            &c::ACCESS_CHANGED,
            format!("access changed from {old} to {new}"),
            none(),
        ),
        K::DefaultChanged { old, new } => (
            &c::DEFAULT_CHANGED,
            format!(
                "default changed from {} to {}",
                old.as_ref()
                    .map_or_else(|| "none".to_owned(), ToString::to_string),
                new.as_ref()
                    .map_or_else(|| "none".to_owned(), ToString::to_string)
            ),
            none(),
        ),
    }
}

fn security_name(requirement: &str) -> String {
    if requirement == "{}" {
        "anonymous".to_owned()
    } else {
        format!("`{requirement}`")
    }
}

/// A stable identifier for a finding: 12 hexadecimal digits of the 64-bit
/// FNV-1a hash of the API name, rule code, path and a kind-specific detail
/// (for example the enum value). It depends on nothing that moves when a file
/// is reformatted, so waivers keep matching. Changing this function
/// invalidates every waiver and baseline in the wild.
pub fn finding_id(api: &str, code: &str, path: &str, detail: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for part in [api, code, path, detail] {
        for byte in part.bytes().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    format!("{:012x}", hash >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface_change(kind: ChangeKind, polarity: Polarity) -> Finding {
        let change = Change {
            kind,
            path: "schema[m].f".parse().unwrap(),
            polarity,
            old_span: None,
            new_span: None,
            effect: None,
        };
        classify("api", &change)
    }

    #[test]
    fn identity_changes_have_fixed_severities() {
        let cases = [
            (
                ChangeKind::FieldTagChanged {
                    name: "f".into(),
                    old: 1,
                    new: 2,
                },
                "FL1026",
                Breaking,
            ),
            (
                ChangeKind::FieldRenamed {
                    old: "f".into(),
                    new: "g".into(),
                    tag: Some(1),
                },
                "FL1025",
                Risky,
            ),
            (
                ChangeKind::EnumValueRenumbered {
                    name: "CAT".into(),
                    old: 1,
                    new: 3,
                },
                "FL1029",
                Breaking,
            ),
            (
                ChangeKind::EnumValueRenamed {
                    number: 1,
                    old: "CAT".into(),
                    new: "FELINE".into(),
                },
                "FL1030",
                Risky,
            ),
            (
                ChangeKind::EncodingChanged {
                    old: Some("varint".into()),
                    new: Some("zigzag".into()),
                },
                "FL1031",
                Breaking,
            ),
            (
                ChangeKind::FieldAliasesChanged {
                    name: "f".into(),
                    removed: Vec::new(),
                    added: vec!["fAlias".into()],
                },
                "FL1027",
                Info,
            ),
            (
                ChangeKind::FieldAliasesChanged {
                    name: "f".into(),
                    removed: vec!["fAlias".into()],
                    added: Vec::new(),
                },
                "FL1027",
                Risky,
            ),
            (
                ChangeKind::StreamingChanged {
                    side: "request".into(),
                    streaming: true,
                },
                "FL3001",
                Breaking,
            ),
        ];
        for (kind, code, severity) in cases {
            for polarity in [Polarity::IN, Polarity::OUT] {
                let finding = surface_change(kind.clone(), polarity);
                assert_eq!(finding.rule.code, code, "{kind:?}");
                assert_eq!(finding.severity, severity, "{kind:?} [{polarity}]");
            }
        }
        let moved = surface_change(
            ChangeKind::FieldAliasesChanged {
                name: "f".into(),
                removed: vec!["a".into()],
                added: vec!["b".into()],
            },
            Polarity::IN,
        );
        assert_eq!(
            moved.message,
            "field `f` is no longer accepted as `a` but as `b`"
        );
    }

    #[test]
    fn finding_ids_are_stable_and_sensitive_to_each_part() {
        let id = finding_id("public", "FL1001", "op[GET /pets]/response[200]", "");
        // Pinned: changing the hash would invalidate waivers users have written.
        assert_eq!(
            id,
            finding_id("public", "FL1001", "op[GET /pets]/response[200]", "")
        );
        assert_eq!(id.len(), 12);
        assert_ne!(
            id,
            finding_id("internal", "FL1001", "op[GET /pets]/response[200]", "")
        );
        assert_ne!(
            id,
            finding_id("public", "FL1002", "op[GET /pets]/response[200]", "")
        );
        assert_ne!(
            id,
            finding_id("public", "FL1001", "op[GET /pets]/response[201]", "")
        );
        assert_ne!(
            id,
            finding_id("public", "FL1001", "op[GET /pets]/response[200]", "x")
        );
        // Parts are delimited: moving a byte between parts changes the id.
        assert_ne!(finding_id("ab", "c", "", ""), finding_id("a", "bc", "", ""));
    }

    #[test]
    fn pinned_finding_id() {
        assert_eq!(
            finding_id("api", "FL2001", "op[DELETE /pets/{}]", ""),
            "48229c430ea7"
        );
    }
}
