//! The well-known types under `google/protobuf/`, and the HTTP annotations
//! under `google/api/`, built in so that imports of them resolve without the
//! files being present in the workspace.
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
        "google/api/annotations.proto" => ANNOTATIONS,
        "google/api/http.proto" => HTTP,
        "google/api/field_behavior.proto" => FIELD_BEHAVIOR,
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

/// `(google.api.http)`: how an RPC is exposed over HTTP/JSON.
const ANNOTATIONS: &str = r#"syntax = "proto3";
package google.api;
import "google/api/http.proto";
import "google/protobuf/descriptor.proto";
extend google.protobuf.MethodOptions {
  HttpRule http = 72295728;
}
"#;

const HTTP: &str = r#"syntax = "proto3";
package google.api;
message Http {
  repeated HttpRule rules = 1;
  bool fully_decode_reserved_expansion = 2;
}
message HttpRule {
  string selector = 1;
  oneof pattern {
    string get = 2;
    string put = 3;
    string post = 4;
    string delete = 5;
    string patch = 6;
    CustomHttpPattern custom = 8;
  }
  string body = 7;
  string response_body = 12;
  repeated HttpRule additional_bindings = 11;
}
message CustomHttpPattern {
  string kind = 1;
  string path = 2;
}
"#;

/// `(google.api.field_behavior)`: `REQUIRED`, `OUTPUT_ONLY` and friends.
const FIELD_BEHAVIOR: &str = r#"syntax = "proto3";
package google.api;
import "google/protobuf/descriptor.proto";
extend google.protobuf.FieldOptions {
  repeated google.api.FieldBehavior field_behavior = 1052;
}
enum FieldBehavior {
  FIELD_BEHAVIOR_UNSPECIFIED = 0;
  OPTIONAL = 1;
  REQUIRED = 2;
  OUTPUT_ONLY = 3;
  INPUT_ONLY = 4;
  IMMUTABLE = 5;
  UNORDERED_LIST = 6;
  NON_EMPTY_DEFAULT = 7;
  IDENTIFIER = 8;
}
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
            "annotations",
            "http",
            "field_behavior",
        ] {
            let text = builtin(&format!("google/protobuf/{path}.proto"))
                .or_else(|| builtin(&format!("google/api/{path}.proto")))
                .unwrap();
            let (_, diagnostics) = parse(SourceId::from_raw(0), text);
            assert!(diagnostics.is_empty(), "{path}: {diagnostics:?}");
        }
        assert!(builtin("google/protobuf/unknown.proto").is_none());
    }
}
