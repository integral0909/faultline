//! The change model: what differs between two versions, where, and with what
//! effect on the values each position admits.

use std::fmt;

use faultline_ir::{IrPath, Literal, ParamLocation, Polarity, StatusKey};
use faultline_source::Span;

/// Evidence about one direction of a value-set change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Evidence {
    /// Proven: no such value exists.
    None,
    /// A concrete value, verified against both versions.
    Witness(Literal),
    /// Could not be decided either way.
    Unknown,
}

impl Evidence {
    pub fn is_none(&self) -> bool {
        matches!(self, Evidence::None)
    }

    pub fn witness(&self) -> Option<&Literal> {
        match self {
            Evidence::Witness(w) => Some(w),
            _ => None,
        }
    }

    /// Whether values may exist in this direction (proven or not ruled out).
    pub fn possible(&self) -> bool {
        !self.is_none()
    }
}

/// How the set of values admitted at a position changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    /// Values the old version admitted and the new one rejects.
    pub lost: Evidence,
    /// Values the new version admits and the old one rejected.
    pub gained: Evidence,
}

impl Effect {
    pub const NONE: Effect = Effect {
        lost: Evidence::None,
        gained: Evidence::None,
    };
}

/// A numeric or size facet of a type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Facet {
    Minimum,
    Maximum,
    MultipleOf,
    MinLength,
    MaxLength,
    MinItems,
    MaxItems,
    MinProperties,
    MaxProperties,
    MinContains,
    MaxContains,
}

impl Facet {
    pub fn keyword(self) -> &'static str {
        match self {
            Facet::Minimum => "minimum",
            Facet::Maximum => "maximum",
            Facet::MultipleOf => "multipleOf",
            Facet::MinLength => "minLength",
            Facet::MaxLength => "maxLength",
            Facet::MinItems => "minItems",
            Facet::MaxItems => "maxItems",
            Facet::MinProperties => "minProperties",
            Facet::MaxProperties => "maxProperties",
            Facet::MinContains => "minContains",
            Facet::MaxContains => "maxContains",
        }
    }
}

/// What changed.
///
/// Surface kinds describe the shape of the API (operations, parameters,
/// responses); type kinds describe the values at a position. The rule
/// catalog classifies every kind, so adding a variant here is a deliberate,
/// compiler-checked extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    OperationAdded,
    OperationRemoved,
    /// An operation is served under another key with an unchanged shape
    /// (a new path, a renamed RPC method).
    OperationMoved {
        to: String,
    },
    OperationIdChanged {
        old: Option<String>,
        new: Option<String>,
    },
    ParameterAdded {
        location: ParamLocation,
        name: String,
        required: bool,
    },
    ParameterRemoved {
        location: ParamLocation,
        name: String,
        required: bool,
    },
    ParameterRequiredChanged {
        location: ParamLocation,
        name: String,
        required: bool,
    },
    RequestBodyAdded {
        required: bool,
    },
    RequestBodyRemoved,
    RequestBodyRequiredChanged {
        required: bool,
    },
    MediaTypeAdded {
        media_type: String,
    },
    MediaTypeRemoved {
        media_type: String,
    },
    /// A message type an operation carries was added (AsyncAPI).
    MessageAdded {
        name: String,
    },
    MessageRemoved {
        name: String,
    },
    ResponseAdded {
        status: StatusKey,
    },
    ResponseRemoved {
        status: StatusKey,
    },
    ResponseHeaderAdded {
        name: String,
        required: bool,
    },
    ResponseHeaderRemoved {
        name: String,
        required: bool,
    },
    ResponseHeaderRequiredChanged {
        name: String,
        required: bool,
    },
    ParameterSerializationChanged {
        location: ParamLocation,
        name: String,
        old: String,
        new: String,
    },
    /// An acceptable combination of security schemes was added. "Anonymous"
    /// (no credentials) counts as a combination.
    SecurityRequirementAdded {
        requirement: String,
    },
    SecurityRequirementRemoved {
        requirement: String,
    },
    WebhookAdded {
        name: String,
    },
    WebhookRemoved {
        name: String,
    },
    CallbackAdded {
        name: String,
    },
    CallbackRemoved {
        name: String,
    },
    SchemaAdded {
        name: String,
    },
    SchemaRemoved {
        name: String,
    },

    /// The value's shape changed (for example string to integer).
    TypeChanged {
        old: String,
        new: String,
    },
    /// A member was declared.
    FieldAdded {
        name: String,
        required: bool,
    },
    /// A member declaration was removed.
    FieldRemoved {
        name: String,
        required: bool,
    },
    FieldRequiredChanged {
        name: String,
        required: bool,
    },
    /// A field keeps its identity (its wire tag, or its old name as an
    /// alias) under a new name.
    FieldRenamed {
        old: String,
        new: String,
        /// The wire tag both names share, or `None` when the fields were
        /// matched through an alias.
        tag: Option<u32>,
    },
    /// A field keeps its name under a new wire tag.
    FieldTagChanged {
        name: String,
        old: u32,
        new: u32,
    },
    /// Names a field is also accepted under were added or removed.
    FieldAliasesChanged {
        name: String,
        removed: Vec<String>,
        added: Vec<String>,
    },
    /// A field moved into, out of, or between exclusive groups, or its
    /// group gained or lost members.
    FieldGroupChanged {
        name: String,
        old: Option<String>,
        new: Option<String>,
    },
    EnumValueAdded {
        value: Literal,
    },
    EnumValueRemoved {
        value: Literal,
    },
    /// An enum value keeps its name under a new number.
    EnumValueRenumbered {
        name: String,
        old: i64,
        new: i64,
    },
    /// An enum number is now spelled differently.
    EnumValueRenamed {
        number: i64,
        old: String,
        new: String,
    },
    /// A field or enum value was removed without retiring its number, so a
    /// later version may reuse it for something else.
    NumberNotRetired {
        name: String,
        number: i64,
    },
    /// A new field or enum value uses a number (or name) an older version
    /// retired: data written under it before is misread.
    RetiredNumberReused {
        name: String,
        number: i64,
    },
    /// Numbers or a name are no longer retired.
    RetirementRemoved {
        /// `number 7`, `numbers 8 to 11`, ``name `legacy` ``.
        what: String,
    },
    /// The wire encoding of a number or collection changed (Protobuf
    /// `int32` to `sint32`, Thrift `list` to `set`): data written one way
    /// is misread the other way.
    EncodingChanged {
        old: Option<String>,
        new: Option<String>,
    },
    /// An RPC direction changed between one message and a stream.
    StreamingChanged {
        /// `request` or `response`.
        side: String,
        streaming: bool,
    },
    /// Members that must appear in order (an XML sequence) appear in a
    /// different order. Only members both versions have are listed.
    MemberOrderChanged {
        old: Vec<String>,
        new: Vec<String>,
    },
    /// An RPC changed between expecting a reply and not (`oneway`).
    OneWayChanged {
        one_way: bool,
    },
    /// A bound or step changed; `None` means absent.
    BoundChanged {
        facet: Facet,
        old: Option<String>,
        new: Option<String>,
    },
    /// Integer became number or vice versa.
    NumericKindChanged {
        integer: bool,
    },
    PatternChanged {
        old: Option<String>,
        new: Option<String>,
    },
    FormatChanged {
        old: Option<String>,
        new: Option<String>,
    },
    UniqueItemsChanged {
        unique: bool,
    },
    VariantAdded {
        index: usize,
    },
    VariantRemoved {
        index: usize,
    },
    UnionModeChanged {
        old: String,
        new: String,
    },
    /// The discriminator property of a union changed (or appeared/vanished).
    DiscriminatorChanged {
        old: Option<String>,
        new: Option<String>,
    },
    DiscriminatorValueAdded {
        value: String,
    },
    DiscriminatorValueRemoved {
        value: String,
    },
    /// A discriminator value now selects a different schema.
    DiscriminatorValueRetargeted {
        value: String,
        old: String,
        new: String,
    },
    /// Constraints expressed through composition (`allOf`, `not`, ...)
    /// changed in a way that has no finer description.
    ConstraintChanged,
    DeprecatedChanged {
        deprecated: bool,
    },
    AccessChanged {
        old: String,
        new: String,
    },
    DefaultChanged {
        old: Option<Literal>,
        new: Option<Literal>,
    },
}

impl ChangeKind {
    /// A stable identifier for the kind, used for ordering and debugging.
    pub fn code(&self) -> &'static str {
        match self {
            ChangeKind::OperationAdded => "operation-added",
            ChangeKind::OperationRemoved => "operation-removed",
            ChangeKind::OperationMoved { .. } => "operation-moved",
            ChangeKind::OperationIdChanged { .. } => "operation-id-changed",
            ChangeKind::ParameterAdded { .. } => "parameter-added",
            ChangeKind::ParameterRemoved { .. } => "parameter-removed",
            ChangeKind::ParameterRequiredChanged { .. } => "parameter-required-changed",
            ChangeKind::RequestBodyAdded { .. } => "request-body-added",
            ChangeKind::RequestBodyRemoved => "request-body-removed",
            ChangeKind::RequestBodyRequiredChanged { .. } => "request-body-required-changed",
            ChangeKind::MediaTypeAdded { .. } => "media-type-added",
            ChangeKind::MediaTypeRemoved { .. } => "media-type-removed",
            ChangeKind::MessageAdded { .. } => "message-added",
            ChangeKind::MessageRemoved { .. } => "message-removed",
            ChangeKind::ResponseAdded { .. } => "response-added",
            ChangeKind::ResponseRemoved { .. } => "response-removed",
            ChangeKind::ResponseHeaderAdded { .. } => "response-header-added",
            ChangeKind::ResponseHeaderRemoved { .. } => "response-header-removed",
            ChangeKind::ResponseHeaderRequiredChanged { .. } => "response-header-required-changed",
            ChangeKind::ParameterSerializationChanged { .. } => "parameter-serialization-changed",
            ChangeKind::SecurityRequirementAdded { .. } => "security-requirement-added",
            ChangeKind::SecurityRequirementRemoved { .. } => "security-requirement-removed",
            ChangeKind::WebhookAdded { .. } => "webhook-added",
            ChangeKind::WebhookRemoved { .. } => "webhook-removed",
            ChangeKind::CallbackAdded { .. } => "callback-added",
            ChangeKind::CallbackRemoved { .. } => "callback-removed",
            ChangeKind::SchemaAdded { .. } => "schema-added",
            ChangeKind::SchemaRemoved { .. } => "schema-removed",
            ChangeKind::TypeChanged { .. } => "type-changed",
            ChangeKind::FieldAdded { .. } => "field-added",
            ChangeKind::FieldRemoved { .. } => "field-removed",
            ChangeKind::FieldRequiredChanged { .. } => "field-required-changed",
            ChangeKind::FieldRenamed { .. } => "field-renamed",
            ChangeKind::FieldTagChanged { .. } => "field-tag-changed",
            ChangeKind::FieldAliasesChanged { .. } => "field-aliases-changed",
            ChangeKind::FieldGroupChanged { .. } => "field-group-changed",
            ChangeKind::EnumValueAdded { .. } => "enum-value-added",
            ChangeKind::EnumValueRemoved { .. } => "enum-value-removed",
            ChangeKind::EnumValueRenumbered { .. } => "enum-value-renumbered",
            ChangeKind::EnumValueRenamed { .. } => "enum-value-renamed",
            ChangeKind::NumberNotRetired { .. } => "number-not-retired",
            ChangeKind::RetiredNumberReused { .. } => "retired-number-reused",
            ChangeKind::RetirementRemoved { .. } => "retirement-removed",
            ChangeKind::EncodingChanged { .. } => "encoding-changed",
            ChangeKind::StreamingChanged { .. } => "streaming-changed",
            ChangeKind::OneWayChanged { .. } => "one-way-changed",
            ChangeKind::MemberOrderChanged { .. } => "member-order-changed",
            ChangeKind::BoundChanged { .. } => "bound-changed",
            ChangeKind::NumericKindChanged { .. } => "numeric-kind-changed",
            ChangeKind::PatternChanged { .. } => "pattern-changed",
            ChangeKind::FormatChanged { .. } => "format-changed",
            ChangeKind::UniqueItemsChanged { .. } => "unique-items-changed",
            ChangeKind::VariantAdded { .. } => "variant-added",
            ChangeKind::VariantRemoved { .. } => "variant-removed",
            ChangeKind::UnionModeChanged { .. } => "union-mode-changed",
            ChangeKind::DiscriminatorChanged { .. } => "discriminator-changed",
            ChangeKind::DiscriminatorValueAdded { .. } => "discriminator-value-added",
            ChangeKind::DiscriminatorValueRemoved { .. } => "discriminator-value-removed",
            ChangeKind::DiscriminatorValueRetargeted { .. } => "discriminator-value-retargeted",
            ChangeKind::ConstraintChanged => "constraint-changed",
            ChangeKind::DeprecatedChanged { .. } => "deprecated-changed",
            ChangeKind::AccessChanged { .. } => "access-changed",
            ChangeKind::DefaultChanged { .. } => "default-changed",
        }
    }
}

/// One difference between the old and the new version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    pub path: IrPath,
    /// The direction values flow at `path`.
    pub polarity: Polarity,
    pub old_span: Option<Span>,
    pub new_span: Option<Span>,
    /// For changes to the values a position admits; `None` for changes to
    /// the API's shape (added operations, removed responses, ...).
    pub effect: Option<Effect>,
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} [{}]", self.kind.code(), self.path, self.polarity)?;
        if let Some(effect) = &self.effect {
            let show = |e: &Evidence| match e {
                Evidence::None => "none".to_owned(),
                Evidence::Witness(w) => w.to_string(),
                Evidence::Unknown => "?".to_owned(),
            };
            write!(
                f,
                " lost={} gained={}",
                show(&effect.lost),
                show(&effect.gained)
            )?;
        }
        Ok(())
    }
}
