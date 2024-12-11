//! Overlays (the OpenAPI Initiative's Overlay Specification 1.0): lists of
//! actions that change a description before it is compared.
//!
//! An overlay document names its actions in `actions`. Each has a JSONPath
//! `target`, evaluated against the document as the earlier actions left
//! it, and either an `update` or `remove: true`:
//!
//! * an update merges into every object the target selects: members the
//!   update has are added, objects on both sides merged member by member,
//!   and other values replaced; into an array it is appended;
//! * a removal takes every selected node out of the object or array
//!   holding it.
//!
//! Values an update brings keep their spans in the overlay document, so
//! findings about them point there. A target that selects nothing is
//! reported as a warning: the overlay no longer matches the description.

use faultline_doc::{Node, Object, Value};
use faultline_jsonpath::ast::Query;
use faultline_jsonpath::{Location, Step};
use faultline_source::{Diagnostic, DiagnosticCode, Diagnostics, Span};

/// The overlay document is malformed, or an action cannot be applied.
pub const INVALID_OVERLAY: DiagnosticCode = DiagnosticCode::new("FLE1201");
/// An action's target is not a valid JSONPath query.
pub const INVALID_TARGET: DiagnosticCode = DiagnosticCode::new("FLE1202");
/// An action's target selects nothing (a warning).
pub const NO_TARGET: DiagnosticCode = DiagnosticCode::new("FLE1203");

/// A read overlay document.
#[derive(Clone, Debug)]
pub struct Overlay {
    pub title: Option<String>,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug)]
pub struct Action {
    pub target: Query,
    /// The target's text, for messages.
    pub target_text: String,
    pub target_span: Span,
    pub change: Change,
}

#[derive(Clone, Debug)]
pub enum Change {
    Update(Node),
    Remove,
}

/// Reads an overlay document, reporting what is wrong with it.
pub fn read(root: &Node, diagnostics: &mut Diagnostics) -> Option<Overlay> {
    let error = |diagnostics: &mut Diagnostics, message: String, span: Span| {
        diagnostics.push(
            Diagnostic::error(message)
                .with_code(INVALID_OVERLAY)
                .with_primary(span, ""),
        );
    };
    let Some(object) = root.as_object() else {
        error(diagnostics, "an overlay is an object".into(), root.span);
        return None;
    };
    match object.get("overlay") {
        Some(Node {
            value: Value::String(version),
            ..
        }) if version.starts_with("1.") => {}
        Some(other) => {
            error(
                diagnostics,
                "only overlays of version 1.x are supported".into(),
                other.span,
            );
            return None;
        }
        None => {
            error(
                diagnostics,
                "an overlay document declares its version in `overlay`".into(),
                root.span,
            );
            return None;
        }
    }
    let title = object
        .get("info")
        .and_then(|i| i.get("title"))
        .and_then(Node::as_str)
        .map(str::to_owned);
    let Some(items) = object.get("actions").and_then(Node::as_array) else {
        error(
            diagnostics,
            "an overlay lists its `actions`".into(),
            root.span,
        );
        return None;
    };
    let mut actions = Vec::with_capacity(items.len());
    let mut valid = true;
    for item in items {
        match action(item, diagnostics) {
            Some(action) => actions.push(action),
            None => valid = false,
        }
    }
    valid.then_some(Overlay { title, actions })
}

fn action(item: &Node, diagnostics: &mut Diagnostics) -> Option<Action> {
    let Some(target) = item.get("target") else {
        diagnostics.push(
            Diagnostic::error("an action names its `target`".to_owned())
                .with_code(INVALID_OVERLAY)
                .with_primary(item.span, ""),
        );
        return None;
    };
    let Some(text) = target.as_str() else {
        diagnostics.push(
            Diagnostic::error("`target` is a JSONPath query".to_owned())
                .with_code(INVALID_TARGET)
                .with_primary(target.span, ""),
        );
        return None;
    };
    let query = match faultline_jsonpath::parse(text) {
        Ok(query) => query,
        Err(e) => {
            diagnostics.push(
                Diagnostic::error(format!("`{text}` is not a JSONPath query: {e}"))
                    .with_code(INVALID_TARGET)
                    .with_primary(target.span, ""),
            );
            return None;
        }
    };
    let remove = item.get("remove").and_then(Node::as_bool) == Some(true);
    let change = match (remove, item.get("update")) {
        (true, _) => Change::Remove,
        (false, Some(update)) => Change::Update(update.clone()),
        (false, None) => {
            diagnostics.push(
                Diagnostic::error("an action has an `update` or `remove: true`".to_owned())
                    .with_code(INVALID_OVERLAY)
                    .with_primary(item.span, ""),
            );
            return None;
        }
    };
    Some(Action {
        target: query,
        target_text: text.to_owned(),
        target_span: target.span,
        change,
    })
}

/// Applies `overlay`'s actions to `document`, in order.
pub fn apply(overlay: &Overlay, document: &mut Node, diagnostics: &mut Diagnostics) {
    for action in &overlay.actions {
        let locations: Vec<Location> = action
            .target
            .select(document)
            .into_iter()
            .map(|m| m.location)
            .collect();
        if locations.is_empty() {
            diagnostics.push(
                Diagnostic::warning(format!("`{}` selects nothing", action.target_text))
                    .with_code(NO_TARGET)
                    .with_primary(action.target_span, ""),
            );
            continue;
        }
        match &action.change {
            Change::Update(update) => {
                for location in &locations {
                    let Some(target) = node_at(document, location) else {
                        continue;
                    };
                    if let Err(problem) = update_node(target, update) {
                        diagnostics.push(
                            Diagnostic::error(format!(
                                "`{}` selects {location}, {problem}",
                                action.target_text
                            ))
                            .with_code(INVALID_OVERLAY)
                            .with_primary(action.target_span, ""),
                        );
                    }
                }
            }
            Change::Remove => remove_all(document, locations, action, diagnostics),
        }
    }
}

/// The node at `location`, if it is still there.
fn node_at<'d>(root: &'d mut Node, location: &Location) -> Option<&'d mut Node> {
    let mut node = root;
    for step in &location.0 {
        node = match (step, &mut node.value) {
            (Step::Name(name), Value::Object(object)) => object.get_mut(name)?,
            (Step::Index(i), Value::Array(items)) => items.get_mut(*i)?,
            _ => return None,
        };
    }
    Some(node)
}

/// Merges `update` into an object, or appends it to an array.
fn update_node(target: &mut Node, update: &Node) -> Result<(), &'static str> {
    match (&mut target.value, &update.value) {
        (Value::Object(into), Value::Object(from)) => {
            merge(into, from);
            Ok(())
        }
        (Value::Array(items), _) => {
            items.push(update.clone());
            Ok(())
        }
        (Value::Object(_), _) => Err("an object, which only an object can update"),
        _ => Err("which is neither an object nor an array"),
    }
}

fn merge(into: &mut Object, from: &Object) {
    for (key, member) in from.members() {
        match into.get_mut(key) {
            Some(existing) => match (&mut existing.value, &member.value.value) {
                (Value::Object(a), Value::Object(b)) => merge(a, b),
                _ => *existing = member.value.clone(),
            },
            None => {
                into.insert(key.to_owned(), member.key_span, member.value.clone());
            }
        }
    }
}

/// Removes the nodes at `locations`, deepest and last first so that the
/// remaining locations stay valid.
fn remove_all(
    document: &mut Node,
    mut locations: Vec<Location>,
    action: &Action,
    diagnostics: &mut Diagnostics,
) {
    locations.sort();
    locations.dedup();
    for location in locations.iter().rev() {
        let Some((last, parent)) = location.0.split_last() else {
            diagnostics.push(
                Diagnostic::error(format!(
                    "`{}` selects the whole document, which cannot be removed",
                    action.target_text
                ))
                .with_code(INVALID_OVERLAY)
                .with_primary(action.target_span, ""),
            );
            continue;
        };
        let Some(parent) = node_at(document, &Location(parent.to_vec())) else {
            continue;
        };
        match (last, &mut parent.value) {
            (Step::Name(name), Value::Object(object)) => {
                object.remove(name);
            }
            (Step::Index(i), Value::Array(items)) if *i < items.len() => {
                items.remove(*i);
            }
            _ => {}
        }
    }
}
