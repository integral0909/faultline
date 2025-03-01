//! RAML 0.8, read as the RAML 1.0 it amounts to.
//!
//! A 0.8 document is rewritten into its 1.0 equivalent before lowering:
//!
//! * `schemas`, `traits`, `resourceTypes` and `securitySchemes` written as
//!   lists of one-member maps become maps;
//! * named parameters (`queryParameters`, `headers`, `formParameters`,
//!   `uriParameters`, `baseUriParameters`) are optional unless they say
//!   `required: true`, except URI parameters, which are always required;
//!   a parameter without a type is a string, `type: date` is a
//!   `datetime`, and `repeat: true` makes an array;
//! * a body's `schema` (the name of a `schemas` entry, or a JSON Schema
//!   written in place) is its type, and `formParameters` make an object of
//!   the form's fields.
//!
//! Schemas in `schemas` are JSON Schemas (or XML Schemas) as text or
//! included files, lowered as RAML 1.0 lowers JSON Schemas used as types.

use faultline_doc::{Node, Object, Value};

/// Keys whose values are named parameters that are optional by default.
const OPTIONAL_PARAMETERS: [&str; 3] = ["queryParameters", "headers", "formParameters"];
/// Keys whose values are named parameters that are required.
const REQUIRED_PARAMETERS: [&str; 2] = ["uriParameters", "baseUriParameters"];
/// Top-level keys that 0.8 may write as lists of one-member maps.
const LISTS: [&str; 4] = ["schemas", "traits", "resourceTypes", "securitySchemes"];

/// Rewrites a RAML 0.8 document into RAML 1.0.
pub(crate) fn upgrade(root: &mut Node) {
    if let Value::Object(object) = &mut root.value {
        for key in LISTS {
            if let Some(value) = object.get_mut(key) {
                flatten(value);
            }
        }
    }
    walk(root);
}

/// `[{a: ..}, {b: ..}]` as `{a: .., b: ..}`.
fn flatten(node: &mut Node) {
    let Value::Array(items) = &node.value else {
        return;
    };
    let mut merged = Object::new();
    for item in items {
        if let Some(object) = item.as_object() {
            for (key, member) in object.members() {
                merged.insert(key.to_owned(), member.key_span, member.value.clone());
            }
        }
    }
    node.value = Value::Object(merged);
}

/// Rewrites named parameters and bodies wherever they are, templates
/// included.
fn walk(node: &mut Node) {
    let Value::Object(object) = &mut node.value else {
        if let Value::Array(items) = &mut node.value {
            items.iter_mut().for_each(walk);
        }
        return;
    };
    for (key, value) in object.iter_mut() {
        let key = key.trim_end_matches('?');
        if OPTIONAL_PARAMETERS.contains(&key) {
            parameters(value, false);
        } else if REQUIRED_PARAMETERS.contains(&key) {
            parameters(value, true);
        } else if key == "body" {
            body(value);
        } else {
            walk(value);
        }
    }
}

fn parameters(node: &mut Node, required: bool) {
    if let Value::Object(parameters) = &mut node.value {
        for (_, parameter) in parameters.iter_mut() {
            named_parameter(parameter, required);
        }
    }
}

/// A named parameter as a RAML 1.0 declaration.
fn named_parameter(node: &mut Node, required: bool) {
    let span = node.span;
    // Several alternative declarations: the first stands for them.
    if let Value::Array(items) = &node.value
        && let Some(first) = items.first().cloned()
    {
        *node = first;
    }
    if node.is_null() {
        node.value = Value::Object(Object::new());
    }
    let Value::Object(object) = &mut node.value else {
        return;
    };
    match object.get("type").and_then(Node::as_str) {
        Some("date") => {
            object.insert(
                "type".into(),
                span,
                Node::new(Value::String("datetime".into()), span),
            );
        }
        // A named parameter without a type is a string.
        None => {
            object.insert(
                "type".into(),
                span,
                Node::new(Value::String("string".into()), span),
            );
        }
        Some(_) => {}
    }
    if !object.contains_key("required") {
        object.insert(
            "required".into(),
            span,
            Node::new(Value::Bool(required), span),
        );
    }
    if object.get("repeat").and_then(Node::as_bool) == Some(true) {
        let mut item = object.clone();
        item.remove("repeat");
        item.remove("required");
        let mut array = Object::new();
        array.insert(
            "type".into(),
            span,
            Node::new(Value::String("array".into()), span),
        );
        array.insert("items".into(), span, Node::new(Value::Object(item), span));
        if let Some(required) = object.get("required").cloned() {
            array.insert("required".into(), span, required);
        }
        *object = array;
    } else {
        object.remove("repeat");
    }
}

/// A body: by media type, or one declaration.
fn body(node: &mut Node) {
    let Value::Object(object) = &mut node.value else {
        return;
    };
    let by_media_type = !object.is_empty() && object.keys().all(|k| k.contains('/'));
    if by_media_type {
        for (_, declaration) in object.iter_mut() {
            body_declaration(declaration);
        }
    } else {
        body_declaration(node);
    }
}

/// A body declaration: `schema` is its type, and `formParameters` the
/// properties of an object.
fn body_declaration(node: &mut Node) {
    let span = node.span;
    let Value::Object(object) = &mut node.value else {
        return;
    };
    if let Some(schema) = object.remove("schema") {
        object.insert("type".into(), schema.key_span, schema.value);
    }
    if let Some(form) = object.remove("formParameters") {
        let mut fields = form.value;
        parameters(&mut fields, false);
        // RAML 1.0 properties are required unless they say otherwise.
        object.insert(
            "type".into(),
            span,
            Node::new(Value::String("object".into()), span),
        );
        object.insert("properties".into(), form.key_span, fields);
    }
}
