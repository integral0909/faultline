//! The well-known types under `google/protobuf/`, built in so that imports of
//! them resolve without the files being present in the workspace.
//!
//! Only the messages, fields and numbers matter for compatibility; the
//! definitions here declare exactly those.

/// The built-in file for an import path, if there is one.
pub(crate) fn builtin(path: &str) -> Option<&'static str> {
    Some(match path {
        "google/protobuf/any.proto" => ANY,
        "google/protobuf/duration.proto" => DURATION,
        "google/protobuf/empty.proto" => EMPTY,
        "google/protobuf/field_mask.proto" => FIELD_MASK,
        "google/protobuf/struct.proto" => STRUCT,
        "google/protobuf/timestamp.proto" => TIMESTAMP,
        "google/protobuf/wrappers.proto" => WRAPPERS,
        "google/protobuf/descriptor.proto" => DESCRIPTOR,
        _ => return None,
    })
}

const ANY: &str = r#"syntax = "proto3";
package google.protobuf;
message Any {
  string type_url = 1;
  bytes value = 2;
}
"#;

const DURATION: &str = r#"syntax = "proto3";
package google.protobuf;
message Duration {
  int64 seconds = 1;
  int32 nanos = 2;
}
"#;

const EMPTY: &str = r#"syntax = "proto3";
package google.protobuf;
message Empty {}
"#;

const FIELD_MASK: &str = r#"syntax = "proto3";
package google.protobuf;
message FieldMask {
  repeated string paths = 1;
}
"#;

const STRUCT: &str = r#"syntax = "proto3";
package google.protobuf;
message Struct {
  map<string, Value> fields = 1;
}
message Value {
  oneof kind {
    NullValue null_value = 1;
    double number_value = 2;
    string string_value = 3;
    bool bool_value = 4;
    Struct struct_value = 5;
    ListValue list_value = 6;
  }
}
enum NullValue {
  NULL_VALUE = 0;
}
message ListValue {
  repeated Value values = 1;
}
"#;

const TIMESTAMP: &str = r#"syntax = "proto3";
package google.protobuf;
message Timestamp {
  int64 seconds = 1;
  int32 nanos = 2;
}
"#;

const WRAPPERS: &str = r#"syntax = "proto3";
package google.protobuf;
message DoubleValue { double value = 1; }
message FloatValue { float value = 1; }
message Int64Value { int64 value = 1; }
message UInt64Value { uint64 value = 1; }
message Int32Value { int32 value = 1; }
message UInt32Value { uint32 value = 1; }
message BoolValue { bool value = 1; }
message StringValue { string value = 1; }
message BytesValue { bytes value = 1; }
"#;

/// The option messages custom options extend. Their own fields do not
/// affect compatibility and are omitted.
const DESCRIPTOR: &str = r#"syntax = "proto2";
package google.protobuf;
message FileOptions { extensions 1000 to max; }
message MessageOptions { extensions 1000 to max; }
message FieldOptions { extensions 1000 to max; }
message OneofOptions { extensions 1000 to max; }
message EnumOptions { extensions 1000 to max; }
message EnumValueOptions { extensions 1000 to max; }
message ServiceOptions { extensions 1000 to max; }
message MethodOptions { extensions 1000 to max; }
message ExtensionRangeOptions { extensions 1000 to max; }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use faultline_source::SourceId;

    #[test]
    fn every_builtin_parses() {
        for path in [
            "any",
            "duration",
            "empty",
            "field_mask",
            "struct",
            "timestamp",
            "wrappers",
            "descriptor",
        ] {
            let text = builtin(&format!("google/protobuf/{path}.proto")).unwrap();
            let (_, diagnostics) = parse(SourceId::from_raw(0), text);
            assert!(diagnostics.is_empty(), "{path}: {diagnostics:?}");
        }
        assert!(builtin("google/protobuf/unknown.proto").is_none());
    }
}
