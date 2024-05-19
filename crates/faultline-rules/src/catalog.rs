//! The rule catalog: every kind of finding Faultline can report.
//!
//! Rule codes are part of the public interface — configuration overrides and
//! documentation refer to them — so a code is never reused or renumbered.
//! `FL1xxx` rules concern the values at a position and apply to every format;
//! `FL2xxx` rules concern HTTP surfaces; `FL3xxx` rules concern RPC surfaces.

use std::fmt;

/// What a compatibility break affects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dimension {
    /// Bytes on the wire: requests or responses stop being accepted or
    /// understood.
    Wire,
    /// Generated code and SDKs: names and signatures change.
    Source,
    /// Documented behaviour: defaults, deprecations.
    Semantic,
}

impl Dimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Dimension::Wire => "wire",
            Dimension::Source => "source",
            Dimension::Semantic => "semantic",
        }
    }
}

/// A rule's static description.
#[derive(Debug, PartialEq, Eq)]
pub struct Rule {
    pub code: &'static str,
    /// Short kebab-case name, stable like the code.
    pub name: &'static str,
    pub title: &'static str,
    pub dimension: Dimension,
    /// Markdown explanation shown by `faultline explain`.
    pub explanation: &'static str,
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.code, self.name)
    }
}

macro_rules! rules {
    ($($ident:ident = $code:literal, $name:literal, $dimension:ident, $title:literal, $explanation:literal;)*) => {
        $(
            pub const $ident: Rule = Rule {
                code: $code,
                name: $name,
                title: $title,
                dimension: Dimension::$dimension,
                explanation: $explanation,
            };
        )*

        /// Every rule, ordered by code.
        pub static RULES: &[&Rule] = &[$(&$ident),*];
    };
}

rules! {
    TYPE_CHANGED = "FL1001", "type-changed", Wire, "The type of a value changed",
        "The value at this position now has a different JSON type or shape, for example a string that became an integer. Values of the old type are rejected (in requests) or unexpected (in responses).\n\nIntroduce the new type alongside the old one, for example as a union, and remove the old one in a later major version.";
    PROPERTY_ADDED = "FL1002", "property-added", Wire, "A property was added",
        "An object gained a declared property. A new *required* property in a request breaks every client that does not send it. In a response, a new property is usually safe, but clients that reject unknown properties may fail.\n\nMake new request properties optional, with a server-side default.";
    PROPERTY_REMOVED = "FL1003", "property-removed", Wire, "A property was removed",
        "An object no longer declares a property. Clients that read it from responses lose data; in requests, the property is now subject to `additionalProperties` and may be rejected.\n\nDeprecate the property first and keep it until the next major version.";
    PROPERTY_REQUIREMENT_CHANGED = "FL1004", "property-requirement-changed", Wire, "A property became required or optional",
        "A property that was optional became required, or the reverse. Making a request property required breaks clients that omit it; making a response property optional breaks clients that rely on its presence.";
    ENUM_VALUE_ADDED = "FL1005", "enum-value-added", Wire, "An enum value was added",
        "An enumeration gained a value. In requests this only widens what is accepted. In responses, clients with exhaustive handling (a `switch` without a default branch, generated enums) may fail on the new value.";
    ENUM_VALUE_REMOVED = "FL1006", "enum-value-removed", Wire, "An enum value was removed",
        "An enumeration lost a value. Requests using it are rejected. In responses this is safe: the value is simply no longer produced.";
    BOUND_CHANGED = "FL1007", "bound-changed", Wire, "A numeric or size constraint changed",
        "A `minimum`, `maximum`, `multipleOf`, length, item-count or property-count constraint changed. Tightening a request constraint rejects values that used to be accepted; loosening a response constraint can produce values clients do not expect (for example strings longer than a fixed-size buffer).";
    NUMERIC_KIND_CHANGED = "FL1008", "numeric-kind-changed", Wire, "An integer became a number or vice versa",
        "Changing `integer` to `number` lets responses carry fractional values that clients parsing integers reject; changing `number` to `integer` rejects fractional request values.";
    PATTERN_CHANGED = "FL1009", "pattern-changed", Wire, "A string pattern changed",
        "The regular expression constraining a string changed. Faultline shows a counterexample when it can decide the change; otherwise the finding is reported as risky.";
    FORMAT_CHANGED = "FL1010", "format-changed", Semantic, "A format annotation changed",
        "The `format` of a value changed, for example `int32` to `int64` or `date` to `date-time`. Formats are annotations in JSON Schema, but generated code and validators often enforce them.";
    UNIQUENESS_CHANGED = "FL1011", "uniqueness-changed", Wire, "Array uniqueness changed",
        "`uniqueItems` was added or removed. Requiring unique items rejects requests with duplicates; allowing duplicates in responses can surprise clients that relied on uniqueness.";
    VARIANT_ADDED = "FL1012", "variant-added", Wire, "A union alternative was added",
        "A union (`anyOf`, `oneOf`, a nullable type) gained an alternative. Responses may now contain values clients do not handle, such as `null`. For `oneOf`, a new alternative can also make previously valid values ambiguous and therefore invalid.";
    VARIANT_REMOVED = "FL1013", "variant-removed", Wire, "A union alternative was removed",
        "A union lost an alternative. Requests using that alternative are rejected.";
    UNION_MODE_CHANGED = "FL1014", "union-mode-changed", Wire, "A union changed between anyOf and oneOf",
        "Switching between `anyOf` and `oneOf` changes whether a value may match several alternatives.";
    COMPOSITION_CHANGED = "FL1015", "composition-changed", Wire, "Composed constraints changed",
        "Constraints expressed through composition (`allOf`, `not`, dependent requirements, pattern properties) changed in a way without a finer description. The severity follows the values Faultline could show to be lost or gained.";
    DISCRIMINATOR_CHANGED = "FL1016", "discriminator-changed", Wire, "A discriminator property changed",
        "The property that selects a union alternative changed, appeared or disappeared. Clients and servers that dispatch on it disagree.";
    DISCRIMINATOR_VALUE_ADDED = "FL1017", "discriminator-value-added", Wire, "A discriminator value was added",
        "A new discriminator value selects an alternative. Responses using it may not be handled by existing clients.";
    DISCRIMINATOR_VALUE_REMOVED = "FL1018", "discriminator-value-removed", Wire, "A discriminator value was removed",
        "A discriminator value no longer selects an alternative. Requests using it are rejected.";
    DISCRIMINATOR_VALUE_RETARGETED = "FL1019", "discriminator-value-retargeted", Wire, "A discriminator value selects a different schema",
        "The same discriminator value now selects a different alternative, so the same payload is interpreted differently.";
    DEPRECATION_CHANGED = "FL1020", "deprecation-changed", Semantic, "Deprecation status changed",
        "An element was deprecated or un-deprecated. Not a break by itself, but a signal to plan removal.";
    ACCESS_CHANGED = "FL1021", "access-changed", Wire, "readOnly or writeOnly changed",
        "A value became read-only or write-only, or stopped being so. Read-only values are ignored in requests and write-only values are never returned, so this changes which direction the value participates in.";
    DEFAULT_CHANGED = "FL1022", "default-changed", Semantic, "A default value changed",
        "The default used when a value is omitted changed. Requests that omit the value now behave differently.";
    SCHEMA_ADDED = "FL1023", "schema-added", Wire, "A schema was added",
        "A standalone schema was added.";
    SCHEMA_REMOVED = "FL1024", "schema-removed", Wire, "A schema was removed",
        "A standalone schema was removed; everything that used it loses its contract.";
    FIELD_RENAMED = "FL1025", "field-renamed", Source, "A field was renamed",
        "A field kept its number (Protobuf tag) but changed its name. The binary encoding identifies fields by number, so binary data still round-trips; but generated code no longer compiles against the old name, and JSON encodings that use field names stop recognising it.\n\nKeep the old name, or add the new field alongside the old one and deprecate the old one.";
    FIELD_NUMBER_CHANGED = "FL1026", "field-number-changed", Wire, "A field's number changed",
        "A field kept its name but moved to another number (Protobuf tag). Binary data written with the old number is read as an unknown field or, worse, as whatever field now has that number.\n\nField numbers are permanent: add a new field instead, and reserve the old number when the old field is removed.";
    FIELD_ALIASES_CHANGED = "FL1027", "field-aliases-changed", Wire, "Alternative names of a field changed",
        "The other names a field is accepted under changed, such as a Protobuf `json_name` or an Avro alias. Data written under a removed name is no longer recognised and the value is silently dropped.";
    FIELD_GROUP_CHANGED = "FL1028", "field-group-changed", Wire, "A field's oneof membership changed",
        "A field moved into or out of a `oneof`, or its `oneof` gained or lost members. Adding a field to a group means messages that set it together with another member are no longer valid: one of the two values is dropped. Removing a field from a group lets receivers see combinations they never saw before.\n\nOnly add *new* fields to an existing `oneof`.";
    ENUM_VALUE_RENUMBERED = "FL1029", "enum-value-renumbered", Wire, "An enum value's number changed",
        "An enum value kept its name but has a new number. Binary data carries the number, so old data now decodes to a different value or an unknown one.\n\nEnum numbers are permanent: add a new value instead.";
    ENUM_VALUE_RENAMED = "FL1030", "enum-value-renamed", Source, "An enum value was renamed",
        "An enum number is now spelled differently. Binary data is unaffected, but generated code and JSON encodings that use value names break.";
    ENCODING_CHANGED = "FL1031", "encoding-changed", Wire, "A number's wire encoding changed",
        "The encoding of a number changed between incompatible representations, for example Protobuf `int32` to `sint32` (zigzag) or `fixed64` to `uint64` (varint). The binary data of one is misread by the other even when the values would fit.\n\nKeep the encoding; changes between `int32`, `uint32`, `int64`, `uint64` and `bool` are compatible on the wire, as are `sint32`/`sint64`, `fixed32`/`sfixed32` and `fixed64`/`sfixed64`, subject to the value ranges, which Faultline checks separately.";

    OPERATION_REMOVED = "FL2001", "operation-removed", Wire, "An operation was removed",
        "An operation (an HTTP method and path, or an RPC method) no longer exists. Clients calling it receive errors.\n\nDeprecate the operation first and remove it in a major version.";
    OPERATION_ADDED = "FL2002", "operation-added", Wire, "An operation was added",
        "A new operation (an HTTP endpoint or an RPC method) was added.";
    OPERATION_ID_CHANGED = "FL2003", "operation-id-changed", Source, "An operationId changed",
        "The `operationId` changed. The wire contract is unaffected, but generated SDKs usually derive method names from it, so client code stops compiling.";
    PARAMETER_ADDED = "FL2010", "parameter-added", Wire, "A parameter was added",
        "An operation gained a parameter. A new *required* parameter breaks every existing client.";
    PARAMETER_REMOVED = "FL2011", "parameter-removed", Wire, "A parameter was removed",
        "An operation no longer declares a parameter. Clients still sending it may be rejected or silently ignored.";
    PARAMETER_REQUIREMENT_CHANGED = "FL2012", "parameter-requirement-changed", Wire, "A parameter became required or optional",
        "Making a parameter required breaks clients that omit it.";
    PARAMETER_SERIALIZATION_CHANGED = "FL2013", "parameter-serialization-changed", Wire, "Parameter serialization changed",
        "The `style`, `explode`, `allowReserved` or `allowEmptyValue` of a parameter changed, so the same value is written differently on the wire (for example `tags=a,b` instead of `tags=a&tags=b`).";
    REQUEST_BODY_ADDED = "FL2020", "request-body-added", Wire, "A request body was added",
        "An operation now accepts a request body. A *required* body breaks clients that send none.";
    REQUEST_BODY_REMOVED = "FL2021", "request-body-removed", Wire, "A request body was removed",
        "An operation no longer accepts a request body. Clients sending one may be rejected.";
    REQUEST_BODY_REQUIREMENT_CHANGED = "FL2022", "request-body-requirement-changed", Wire, "A request body became required or optional",
        "Making the request body required breaks clients that send none.";
    MEDIA_TYPE_ADDED = "FL2030", "media-type-added", Wire, "A media type was added",
        "A request or response gained a media type. Additional request media types are safe; additional response media types only matter to clients that do not send `Accept`.";
    MEDIA_TYPE_REMOVED = "FL2031", "media-type-removed", Wire, "A media type was removed",
        "A request or response no longer supports a media type. Clients sending it (requests) or asking for it (responses) fail.";
    RESPONSE_ADDED = "FL2040", "response-added", Wire, "A response status was added",
        "An operation documents a new response status. Clients with exhaustive status handling may not expect it.";
    RESPONSE_REMOVED = "FL2041", "response-removed", Wire, "A response status was removed",
        "An operation no longer documents a response status. Clients are not affected by responses they no longer receive, but the change may reflect changed behaviour.";
    RESPONSE_HEADER_ADDED = "FL2050", "response-header-added", Wire, "A response header was added",
        "A response gained a header.";
    RESPONSE_HEADER_REMOVED = "FL2051", "response-header-removed", Wire, "A response header was removed",
        "A response no longer includes a header. Clients reading it lose information.";
    RESPONSE_HEADER_REQUIREMENT_CHANGED = "FL2052", "response-header-requirement-changed", Wire, "A response header became required or optional",
        "A header that was always sent may now be missing.";
    SECURITY_REQUIREMENT_ADDED = "FL2060", "security-requirement-added", Wire, "A security alternative was added",
        "An operation accepts an additional combination of credentials. Existing clients are unaffected.";
    SECURITY_REQUIREMENT_REMOVED = "FL2061", "security-requirement-removed", Wire, "A security alternative was removed",
        "An operation no longer accepts a combination of credentials. When the removed combination is anonymous access, the operation now requires authentication.";
    CALLBACK_ADDED = "FL2070", "callback-added", Wire, "A callback was added",
        "An operation now makes callback requests to the client.";
    CALLBACK_REMOVED = "FL2071", "callback-removed", Wire, "A callback was removed",
        "An operation no longer makes a callback; clients waiting for it are affected.";
    WEBHOOK_ADDED = "FL2072", "webhook-added", Wire, "A webhook was added",
        "The API sends a new webhook.";
    WEBHOOK_REMOVED = "FL2073", "webhook-removed", Wire, "A webhook was removed",
        "The API no longer sends a webhook; receivers depending on it are affected.";

    STREAMING_CHANGED = "FL3001", "streaming-changed", Wire, "An RPC changed between unary and streaming",
        "A request or response of a remote procedure became a stream of messages, or stopped being one. Clients and servers disagree on the call's shape, and every call fails.\n\nAdd a new method with the new shape and deprecate the old one.";
}

/// Looks up a rule by code (`FL1001`) or name (`type-changed`),
/// case-insensitively.
pub fn find(code_or_name: &str) -> Option<&'static Rule> {
    RULES.iter().copied().find(|r| {
        r.code.eq_ignore_ascii_case(code_or_name) || r.name.eq_ignore_ascii_case(code_or_name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_names_are_unique_and_ordered() {
        for (i, rule) in RULES.iter().enumerate() {
            assert!(
                rule.code.starts_with("FL") && rule.code.len() == 6,
                "{rule}"
            );
            for other in &RULES[..i] {
                assert!(other.code < rule.code, "{other} must come before {rule}");
                assert_ne!(other.name, rule.name);
            }
        }
    }

    #[test]
    fn lookup_by_code_or_name() {
        assert_eq!(find("fl1001").unwrap().name, "type-changed");
        assert_eq!(find("operation-removed").unwrap().code, "FL2001");
        assert!(find("FL9999").is_none());
    }
}
