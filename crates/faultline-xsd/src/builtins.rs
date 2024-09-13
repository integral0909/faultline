//! The built-in datatypes of XML Schema, as IR kinds.
//!
//! XML carries every value as text; the kinds describe the values each
//! datatype's lexical space denotes, so that `xs:int` to `xs:long` widens a
//! range and `xs:string` to `xs:int` changes the type, as for JSON.

use faultline_doc::Decimal;
use faultline_ir::{ArrayType, Bound, NumericType, StringType, TypeKind};

use crate::reader::XS;

/// The kind of the built-in datatype `local` (in the XML Schema
/// namespace), or `None` if there is no such datatype. `alloc` allocates the
/// string items of the list datatypes.
pub(crate) fn builtin(
    local: &str,
    alloc: &mut dyn FnMut(TypeKind) -> faultline_ir::TypeId,
) -> Option<TypeKind> {
    let string = |format: Option<&str>| {
        TypeKind::String(StringType {
            format: format.map(str::to_owned),
            ..StringType::default()
        })
    };
    let integer = |min: Option<i128>, max: Option<i128>, format: Option<&str>| {
        TypeKind::Integer(NumericType {
            minimum: min.map(|m| Bound::inclusive(Decimal::from_i128(m))),
            maximum: max.map(|m| Bound::inclusive(Decimal::from_i128(m))),
            format: format.map(str::to_owned),
            ..NumericType::default()
        })
    };
    let unsigned = |bits: u32| Some((1i128 << bits) - 1);
    let signed = |bits: u32| {
        (
            Some(-(1i128 << (bits - 1))),
            Some((1i128 << (bits - 1)) - 1),
        )
    };
    Some(match local {
        "anyType" | "anySimpleType" | "anyAtomicType" => TypeKind::Any,
        "string" | "normalizedString" | "token" | "language" | "Name" | "NCName" | "NMTOKEN"
        | "ID" | "IDREF" | "ENTITY" | "QName" | "NOTATION" => string(None),
        "anyURI" => string(Some("uri")),
        "NMTOKENS" | "IDREFS" | "ENTITIES" => {
            let item = alloc(string(None));
            TypeKind::Array(ArrayType {
                min_items: 1,
                ..ArrayType::of(item)
            })
        }
        "boolean" => TypeKind::Boolean,
        "decimal" => TypeKind::Number(NumericType::default()),
        "float" | "double" => TypeKind::Number(NumericType {
            format: Some(local.to_owned()),
            ..NumericType::default()
        }),
        "integer" => integer(None, None, None),
        "long" => {
            let (min, max) = signed(64);
            integer(min, max, Some("int64"))
        }
        "int" => {
            let (min, max) = signed(32);
            integer(min, max, Some("int32"))
        }
        "short" => {
            let (min, max) = signed(16);
            integer(min, max, None)
        }
        "byte" => {
            let (min, max) = signed(8);
            integer(min, max, None)
        }
        "nonNegativeInteger" => integer(Some(0), None, None),
        "positiveInteger" => integer(Some(1), None, None),
        "nonPositiveInteger" => integer(None, Some(0), None),
        "negativeInteger" => integer(None, Some(-1), None),
        "unsignedLong" => integer(Some(0), unsigned(64), None),
        "unsignedInt" => integer(Some(0), unsigned(32), None),
        "unsignedShort" => integer(Some(0), unsigned(16), None),
        "unsignedByte" => integer(Some(0), unsigned(8), None),
        "dateTime" | "dateTimeStamp" => string(Some("date-time")),
        "date" => string(Some("date")),
        "time" => string(Some("time")),
        "duration" | "dayTimeDuration" | "yearMonthDuration" => string(Some("duration")),
        "gYear" | "gYearMonth" | "gMonth" | "gMonthDay" | "gDay" => string(Some(local)),
        "base64Binary" => string(Some("byte")),
        "hexBinary" => string(Some("hex")),
        _ => return None,
    })
}

/// Whether `namespace` is the XML Schema namespace.
pub(crate) fn is_xs(namespace: Option<&str>) -> bool {
    namespace == Some(XS)
}
