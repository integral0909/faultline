//! Where subschemas live inside a schema object.
//!
//! Keyword semantics depend on position: `properties/$id` is a property named
//! `$id`, not an identifier. Scanning for identifiers and anchors must
//! therefore visit exactly the subschema positions of each keyword, which this
//! module enumerates per dialect.

use faultline_doc::{Node, Object, Value};

use crate::dialect::Dialect;

/// Keywords whose value is a single subschema.
fn single_keywords(dialect: Dialect) -> impl Iterator<Item = &'static str> {
    let mut keywords = vec!["additionalProperties", "not"];
    if dialect.has_draft6_keywords() {
        keywords.extend(["propertyNames", "contains"]);
    }
    if dialect.has_conditionals() {
        keywords.extend(["if", "then", "else"]);
    }
    if dialect.has_2019_keywords() {
        keywords.extend(["unevaluatedProperties", "unevaluatedItems"]);
    }
    if !dialect.has_prefix_items() {
        keywords.push("additionalItems");
    }
    keywords.into_iter()
}

/// Keywords whose value is an array of subschemas.
fn array_keywords(dialect: Dialect) -> impl Iterator<Item = &'static str> {
    let mut keywords = vec!["allOf", "anyOf", "oneOf"];
    if dialect.has_prefix_items() {
        keywords.push("prefixItems");
    }
    keywords.into_iter()
}

/// Keywords whose value is an object mapping names to subschemas.
fn map_keywords(dialect: Dialect) -> impl Iterator<Item = &'static str> {
    // `definitions` is not a keyword after draft-07 but remains the most common
    // place for reusable schemas in 2019-09+ documents, and references into
    // it must still find identifiers declared there.
    let mut keywords = vec!["properties", "patternProperties", "definitions"];
    if dialect.has_2019_keywords() {
        keywords.extend(["$defs", "dependentSchemas"]);
    } else {
        keywords.push("dependencies");
    }
    keywords.into_iter()
}

/// Whether `node` can be a schema in `dialect`.
pub(crate) fn is_schema_value(node: &Node, dialect: Dialect) -> bool {
    match node.value {
        Value::Object(_) => true,
        Value::Bool(_) => dialect.has_boolean_schemas(),
        _ => false,
    }
}

/// Calls `visit(tokens, subschema)` for every immediate subschema of `schema`,
/// where `tokens` is the relative JSON Pointer path from `schema`.
pub(crate) fn for_each_subschema<'a>(
    schema: &'a Object,
    dialect: Dialect,
    mut visit: impl FnMut(Vec<String>, &'a Node),
) {
    let mut emit = |tokens: Vec<String>, node: &'a Node| {
        if is_schema_value(node, dialect) {
            visit(tokens, node);
        }
    };
    for keyword in single_keywords(dialect) {
        if let Some(node) = schema.get(keyword) {
            emit(vec![keyword.to_owned()], node);
        }
    }
    // `items` is a single schema, or (before 2020-12) an array of them.
    if let Some(items) = schema.get("items") {
        match (&items.value, dialect.has_prefix_items()) {
            (Value::Array(list), false) => {
                for (i, node) in list.iter().enumerate() {
                    emit(vec!["items".into(), i.to_string()], node);
                }
            }
            _ => emit(vec!["items".into()], items),
        }
    }
    for keyword in array_keywords(dialect) {
        if let Some(list) = schema.get(keyword).and_then(Node::as_array) {
            for (i, node) in list.iter().enumerate() {
                emit(vec![keyword.to_owned(), i.to_string()], node);
            }
        }
    }
    for keyword in map_keywords(dialect) {
        if let Some(map) = schema.get(keyword).and_then(Node::as_object) {
            for (name, node) in map.iter() {
                emit(vec![keyword.to_owned(), name.to_owned()], node);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faultline_doc::{Format, parse};
    use faultline_source::{Limits, SourceMap, SourceName};

    fn paths(json: &str, dialect: Dialect) -> Vec<String> {
        let map = SourceMap::new();
        let file = map.add(SourceName::Virtual("t".into()), json).unwrap();
        let doc = parse(&file, Format::Json, &Limits::default())
            .document
            .unwrap();
        let mut out = Vec::new();
        for_each_subschema(doc.root.as_object().unwrap(), dialect, |tokens, _| {
            out.push(tokens.join("/"));
        });
        out.sort();
        out
    }

    #[test]
    fn positions_depend_on_dialect() {
        let schema = r#"{
            "properties": {"$id": {"type": "string"}, "flag": true},
            "items": [{}, {}],
            "prefixItems": [{}],
            "dependencies": {"a": ["b"], "c": {}},
            "dependentSchemas": {"d": {}},
            "if": {}, "enum": [{}], "const": {}
        }"#;
        assert_eq!(
            paths(schema, Dialect::Draft7),
            [
                "dependencies/c",
                "if",
                "items/0",
                "items/1",
                "properties/$id",
                "properties/flag"
            ]
        );
        assert_eq!(
            paths(schema, Dialect::Draft4),
            ["dependencies/c", "items/0", "items/1", "properties/$id"],
            "no conditionals and no boolean schemas in draft-04"
        );
        assert_eq!(
            paths(schema, Dialect::Draft2020_12),
            [
                "dependentSchemas/d",
                "if",
                "prefixItems/0",
                "properties/$id",
                "properties/flag"
            ],
            "array-valued items is not a schema position in 2020-12"
        );
    }
}
