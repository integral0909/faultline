//! Compact JSON serialization of document nodes.
//!
//! Used wherever Faultline shows a value to a user or machine: witness values
//! in findings, enum members in messages, and the JSON report.

use std::fmt::Write as _;

use crate::node::{Node, Value};
use crate::number::Number;

impl Node {
    /// Serializes the value as compact JSON.
    ///
    /// Member order is preserved. YAML's non-finite numbers have no JSON
    /// representation and are written as the strings `".inf"`, `"-.inf"` and
    /// `".nan"`, so the output is always valid JSON.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        write_value(&self.value, &mut out);
        out
    }
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(Number::Finite(d)) => {
            let _ = write!(out, "{d}");
        }
        Value::Number(special) => write_json_string(&special.to_string(), out),
        Value::String(s) => write_json_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(&item.value, out);
            }
            out.push(']');
        }
        Value::Object(obj) => {
            out.push('{');
            for (i, (key, value)) in obj.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_string(key, out);
                out.push(':');
                write_value(&value.value, out);
            }
            out.push('}');
        }
    }
}

/// Writes a JSON string literal. Non-ASCII characters are written as-is;
/// only the characters JSON requires to be escaped are escaped.
pub fn write_json_string(s: &str, out: &mut String) {
    out.reserve(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use faultline_source::{SourceId, Span};

    use crate::node::{Node, Object, Value};
    use crate::number::{Decimal, Number};

    fn node(value: Value) -> Node {
        Node::new(value, Span::empty(SourceId::from_raw(0), 0))
    }

    #[test]
    fn writes_compact_json_in_member_order() {
        let mut obj = Object::new();
        let span = Span::empty(SourceId::from_raw(0), 0);
        obj.insert(
            "z".into(),
            span,
            node(Value::Number(Decimal::parse("1.50").unwrap().into())),
        );
        obj.insert(
            "a".into(),
            span,
            node(Value::Array(vec![
                node(Value::Null),
                node(Value::Bool(false)),
            ])),
        );
        assert_eq!(
            node(Value::Object(obj)).to_json(),
            r#"{"z":1.5,"a":[null,false]}"#
        );
    }

    #[test]
    fn escapes_only_what_json_requires() {
        let s = node(Value::String("q\"b\\n\n\u{1}é".into()));
        assert_eq!(s.to_json(), "\"q\\\"b\\\\n\\n\\u0001é\"");
    }

    #[test]
    fn non_finite_numbers_become_strings() {
        assert_eq!(
            node(Value::Number(Number::NegInfinity)).to_json(),
            "\"-.inf\""
        );
    }
}
