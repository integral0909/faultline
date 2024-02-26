use std::collections::BTreeMap;
use std::fmt;

use faultline_doc::{Node, Number, Value, write_json_string};

/// A constant value in a contract: an enum member, a `const`, a default.
///
/// Unlike a document [`Node`], a literal carries no spans and its equality
/// is semantic: object members are kept sorted by key and numbers are exact
/// decimals, so derived `Eq` and `Hash` agree with JSON value equality.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Literal {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Literal>),
    Object(BTreeMap<String, Literal>),
}

impl Literal {
    pub fn string(s: impl Into<String>) -> Self {
        Literal::String(s.into())
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Literal::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<&Number> {
        match self {
            Literal::Number(n) => Some(n),
            _ => None,
        }
    }

    /// A short name for the JSON type of the value, as used in messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Literal::Null => "null",
            Literal::Bool(_) => "boolean",
            Literal::Number(_) => "number",
            Literal::String(_) => "string",
            Literal::Array(_) => "array",
            Literal::Object(_) => "object",
        }
    }
}

impl From<&Node> for Literal {
    fn from(node: &Node) -> Self {
        match &node.value {
            Value::Null => Literal::Null,
            Value::Bool(b) => Literal::Bool(*b),
            Value::Number(n) => Literal::Number(n.clone()),
            Value::String(s) => Literal::String(s.clone()),
            Value::Array(items) => Literal::Array(items.iter().map(Literal::from).collect()),
            Value::Object(obj) => Literal::Object(
                obj.iter()
                    .map(|(k, v)| (k.to_owned(), Literal::from(v)))
                    .collect(),
            ),
        }
    }
}

impl fmt::Display for Literal {
    /// Compact JSON. Non-finite numbers, which only YAML can express, are
    /// written as their YAML spelling inside a string.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        write_literal(self, &mut out);
        f.write_str(&out)
    }
}

fn write_literal(literal: &Literal, out: &mut String) {
    match literal {
        Literal::Null => out.push_str("null"),
        Literal::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Literal::Number(Number::Finite(d)) => out.push_str(&d.to_string()),
        Literal::Number(other) => write_json_string(&other.to_string(), out),
        Literal::String(s) => write_json_string(s, out),
        Literal::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_literal(item, out);
            }
            out.push(']');
        }
        Literal::Object(members) => {
            out.push('{');
            for (i, (key, value)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_string(key, out);
                out.push(':');
                write_literal(value, out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faultline_doc::{Format, parse};
    use faultline_source::{Limits, SourceMap, SourceName};

    fn lit(json: &str) -> Literal {
        let map = SourceMap::new();
        let file = map.add(SourceName::Virtual("t".into()), json).unwrap();
        let doc = parse(&file, Format::Json, &Limits::default())
            .document
            .unwrap();
        Literal::from(&doc.root)
    }

    #[test]
    fn equality_is_semantic() {
        assert_eq!(
            lit(r#"{"b": 1.0, "a": [true]}"#),
            lit(r#"{"a": [true], "b": 1}"#)
        );
        assert_ne!(lit("[1, 2]"), lit("[2, 1]"));
        assert_ne!(lit("1"), lit(r#""1""#));
    }

    #[test]
    fn displays_as_compact_json_with_sorted_keys() {
        assert_eq!(
            lit(r#"{"z": "q\"", "a": [1e2, null]}"#).to_string(),
            r#"{"a":[100,null],"z":"q\""}"#
        );
    }
}
