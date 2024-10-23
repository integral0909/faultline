//! Resource types and traits: templates merged into resources and methods.
//!
//! A resource names a resource type in `type` (`type: collection`, or
//! `type: {collection: {item: Pet}}` with parameters); a resource or method
//! names traits in `is`. Each template's `<<parameter>>`s are replaced — by
//! the parameters given, and by `<<resourcePath>>`, `<<resourcePathName>>`
//! and `<<methodName>>` — optionally transformed (`<<item | !pluralize>>`),
//! and the result is merged into the resource or method: values it already
//! has win, maps are merged member by member, and methods a resource type
//! declares as optional (`get?:`) are only merged into methods the resource
//! has.
//!
//! Precedence, highest first: the method itself, the method's traits in
//! the order listed, the resource's traits, the resource type (and in turn
//! its own type).

use std::collections::HashMap;

use faultline_doc::{Node, Object, Value};
use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::codes;

/// Resource types inherit from each other at most this deep.
const MAX_TYPE_DEPTH: usize = 16;

/// The HTTP methods a resource may declare.
const METHOD_KEYS: [&str; 7] = ["get", "put", "post", "delete", "options", "head", "patch"];

/// The templates a document declares, and those of the libraries it uses.
#[derive(Clone, Debug, Default)]
pub struct Templates {
    pub resource_types: HashMap<String, Node>,
    pub traits: HashMap<String, Node>,
    pub libraries: HashMap<String, Templates>,
}

impl Templates {
    /// The `resourceTypes` and `traits` of a document.
    pub fn of(root: &Node) -> Templates {
        let collect = |key: &str| -> HashMap<String, Node> {
            root.get(key)
                .and_then(Node::as_object)
                .map(|o| o.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect())
                .unwrap_or_default()
        };
        Templates {
            resource_types: collect("resourceTypes"),
            traits: collect("traits"),
            libraries: HashMap::new(),
        }
    }

    /// Qualifies the template references in this library's own templates
    /// with `prefix` (the library's name and a dot), so that they still
    /// name the library's templates once borrowed by a document that
    /// names them differently.
    pub fn qualify(&mut self, prefix: &str) {
        let known = self.clone();
        let qualify = |node: &mut Node, traits: bool| {
            qualify_references(node, traits, prefix, &known);
        };
        for template in self.resource_types.values_mut() {
            let Value::Object(object) = &mut template.value else {
                continue;
            };
            for (key, member) in object.iter_mut() {
                match key {
                    "type" => qualify(member, false),
                    "is" => qualify(member, true),
                    _ if METHOD_KEYS.contains(&key.trim_end_matches('?')) => {
                        if let Value::Object(method) = &mut member.value {
                            for (key, traits) in method.iter_mut() {
                                if key == "is" {
                                    qualify(traits, true);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// The resource type or trait (`traits` picks which) a possibly
    /// qualified name (`lib.paged`) refers to.
    fn find(&self, name: &str, traits: bool) -> Option<&Node> {
        let own = if traits {
            &self.traits
        } else {
            &self.resource_types
        };
        if let Some(template) = own.get(name) {
            return Some(template);
        }
        let (library, rest) = name.split_once('.')?;
        self.libraries.get(library)?.find(rest, traits)
    }
}

/// Applies resource types and traits to resources.
#[derive(Debug)]
pub struct Expander<'t> {
    templates: &'t Templates,
    pub diagnostics: Diagnostics,
}

impl<'t> Expander<'t> {
    pub fn new(templates: &'t Templates) -> Self {
        Self {
            templates,
            diagnostics: Diagnostics::new(),
        }
    }

    fn error(&mut self, message: String, span: Span) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(codes::UNRESOLVED)
                .with_primary(span, ""),
        );
    }

    /// `resource` at `path` with its resource type and traits applied.
    pub fn resource(&mut self, path: &str, resource: &Node) -> Node {
        let mut result = resource.clone();
        let path_name = resource_path_name(path);
        let builtins = HashMap::from([
            ("resourcePath".to_owned(), path.to_owned()),
            ("resourcePathName".to_owned(), path_name),
        ]);
        // The resource type, and the types it in turn names.
        let mut next = resource.get("type").cloned();
        let mut depth = 0;
        while let Some(reference) = next.take() {
            depth += 1;
            if depth > MAX_TYPE_DEPTH {
                self.error(
                    "resource types inherit too deeply (a type may name itself)".into(),
                    reference.span,
                );
                break;
            }
            let Some((template, parameters)) = self.lookup(&reference, false) else {
                break;
            };
            let mut parameters = parameters;
            parameters.extend(builtins.clone());
            let applied = substitute(&template, &parameters);
            next = applied.get("type").cloned();
            merge_resource(&mut result, &applied);
        }
        // Traits of the resource apply to each of its methods.
        let resource_traits = result.get("is").cloned();
        if let Value::Object(object) = &mut result.value {
            for (key, method) in object.iter_mut() {
                if !METHOD_KEYS.contains(&key) || method.as_object().is_none() {
                    continue;
                }
                let mut parameters = builtins.clone();
                parameters.insert("methodName".to_owned(), key.to_owned());
                let own = method.get("is").cloned();
                let mut applied = method.clone();
                for traits in [own, resource_traits.clone()].into_iter().flatten() {
                    self.apply_traits(&mut applied, &traits, &parameters);
                }
                *method = applied;
            }
        }
        result
    }

    /// Merges each trait `is` names into `method`, in order.
    fn apply_traits(&mut self, method: &mut Node, list: &Node, builtins: &HashMap<String, String>) {
        let references: Vec<Node> = match &list.value {
            Value::Array(items) => items.clone(),
            _ => vec![list.clone()],
        };
        for reference in &references {
            let Some((template, mut parameters)) = self.lookup(reference, true) else {
                continue;
            };
            parameters.extend(builtins.clone());
            merge(method, &substitute(&template, &parameters));
        }
    }

    /// The template a reference names (`name`, or `{name: {param: value}}`)
    /// and the parameters it passes.
    fn lookup(
        &mut self,
        reference: &Node,
        traits: bool,
    ) -> Option<(Node, HashMap<String, String>)> {
        let what = if traits { "trait" } else { "resource type" };
        let (name, parameters) = match &reference.value {
            Value::String(name) => (name.clone(), HashMap::new()),
            Value::Object(object) if object.len() == 1 => {
                let (name, params) = object.iter().next().expect("one member");
                let parameters = params
                    .as_object()
                    .map(|p| {
                        p.iter()
                            .map(|(k, v)| (k.to_owned(), scalar_text(v)))
                            .collect()
                    })
                    .unwrap_or_default();
                (name.to_owned(), parameters)
            }
            _ => {
                self.error(
                    format!("a {what} is named by a name or a one-member map"),
                    reference.span,
                );
                return None;
            }
        };
        if let Some(template) = self.templates.find(&name, traits) {
            return Some((template.clone(), parameters));
        }
        self.error(format!("{what} {name} is not declared"), reference.span);
        None
    }
}

/// Prefixes the names in a template reference (or list of them, for `is`)
/// that `known` declares.
fn qualify_references(node: &mut Node, traits: bool, prefix: &str, known: &Templates) {
    match &mut node.value {
        Value::Array(items) => {
            for item in items {
                qualify_references(item, traits, prefix, known);
            }
        }
        Value::String(name) => {
            if known.find(name, traits).is_some() {
                *name = format!("{prefix}{name}");
            }
        }
        Value::Object(object) if object.len() == 1 => {
            let (name, member) = object.members().next().expect("one member");
            if known.find(name, traits).is_some() {
                let mut renamed = Object::new();
                renamed.insert(
                    format!("{prefix}{name}"),
                    member.key_span,
                    member.value.clone(),
                );
                *object = renamed;
            }
        }
        _ => {}
    }
}

/// The text of a scalar parameter value.
fn scalar_text(node: &Node) -> String {
    match &node.value {
        Value::String(s) => s.clone(),
        _ => faultline_ir::Literal::from(node).to_string(),
    }
}

/// The last literal segment of a resource path: `/pets/{petId}` gives
/// `pets`, as RAML specifies.
fn resource_path_name(path: &str) -> String {
    path.split('/')
        .rev()
        .find(|s| !s.is_empty() && !s.starts_with('{'))
        .unwrap_or_default()
        .to_owned()
}

/// Merges a resource type into a resource: methods declared optional
/// (`get?`) only into methods the resource has.
fn merge_resource(resource: &mut Node, template: &Node) {
    let Some(members) = template.as_object() else {
        return;
    };
    let Value::Object(target) = &mut resource.value else {
        return;
    };
    for (key, member) in members.members() {
        if key == "type" {
            continue;
        }
        if let Some(method) = key.strip_suffix('?').filter(|m| METHOD_KEYS.contains(m)) {
            if let Some(existing) = target_get_mut(target, method) {
                merge(existing, &member.value);
            }
            continue;
        }
        match target_get_mut(target, key) {
            Some(existing) => merge(existing, &member.value),
            None => {
                target.insert(key.to_owned(), member.key_span, member.value.clone());
            }
        }
    }
}

fn target_get_mut<'o>(object: &'o mut Object, key: &str) -> Option<&'o mut Node> {
    object.iter_mut().find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// Merges `source` into `target`: members `target` lacks are added, maps
/// present in both are merged, an empty `target` takes `source`, and
/// anything else keeps `target`'s value.
pub(crate) fn merge(target: &mut Node, source: &Node) {
    // An empty declaration (`delete:`) takes the template's.
    if target.is_null() {
        target.value = source.value.clone();
        return;
    }
    let (Value::Object(into), Some(from)) = (&mut target.value, source.as_object()) else {
        return;
    };
    for (key, member) in from.members() {
        match target_get_mut(into, key) {
            Some(existing) => merge(existing, &member.value),
            None => {
                into.insert(key.to_owned(), member.key_span, member.value.clone());
            }
        }
    }
}

/// `node` with every `<<parameter>>` in its keys and strings replaced.
pub(crate) fn substitute(node: &Node, parameters: &HashMap<String, String>) -> Node {
    let value = match &node.value {
        Value::String(text) => Value::String(replace(text, parameters)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|i| substitute(i, parameters)).collect())
        }
        Value::Object(object) => {
            let mut out = Object::new();
            for (key, member) in object.members() {
                out.insert(
                    replace(key, parameters),
                    member.key_span,
                    substitute(&member.value, parameters),
                );
            }
            Value::Object(out)
        }
        other => other.clone(),
    };
    Node::new(value, node.span)
}

/// Replaces `<<name>>` and `<<name | !transform>>` in `text`. Unknown
/// parameters are left as written.
fn replace(text: &str, parameters: &HashMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<<") {
        let Some(end) = rest[start..].find(">>") else {
            break;
        };
        out.push_str(&rest[..start]);
        let inner = &rest[start + 2..start + end];
        let mut pieces = inner.split('|').map(str::trim);
        let name = pieces.next().unwrap_or_default();
        match parameters.get(name) {
            Some(value) => {
                let mut value = value.clone();
                for transform in pieces {
                    value = transformed(&value, transform.trim_start_matches('!'));
                }
                out.push_str(&value);
            }
            None => out.push_str(&rest[start..start + end + 2]),
        }
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    out
}

/// RAML's parameter transformations.
fn transformed(value: &str, transform: &str) -> String {
    let words = words(value);
    match transform {
        "singularize" => singular(value),
        "pluralize" => plural(value),
        "uppercase" => value.to_uppercase(),
        "lowercase" => value.to_lowercase(),
        "lowercamelcase" => camel(&words, false),
        "uppercamelcase" => camel(&words, true),
        "lowerunderscorecase" => words.join("_").to_lowercase(),
        "upperunderscorecase" => words.join("_").to_uppercase(),
        "lowerhyphencase" => words.join("-").to_lowercase(),
        "upperhyphencase" => words.join("-").to_uppercase(),
        _ => value.to_owned(),
    }
}

/// The words of an identifier in any of the cases above.
fn words(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for c in value.chars() {
        if c == '_' || c == '-' || c == ' ' {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if c.is_uppercase() && previous_lower && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        current.push(c);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn camel(words: &[String], upper_first: bool) -> String {
    words
        .iter()
        .enumerate()
        .map(|(i, word)| {
            let lower = word.to_lowercase();
            if i == 0 && !upper_first {
                return lower;
            }
            let mut chars = lower.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        })
        .collect()
}

fn singular(word: &str) -> String {
    if let Some(stem) = word.strip_suffix("ies") {
        format!("{stem}y")
    } else if let Some(stem) = ["sses", "shes", "ches", "xes", "zes"]
        .iter()
        .find_map(|suffix| word.strip_suffix(suffix).map(|s| (s, suffix)))
        .map(|(stem, suffix)| format!("{stem}{}", &suffix[..suffix.len() - 2]))
    {
        stem
    } else if word.ends_with('s') && !word.ends_with("ss") {
        word[..word.len() - 1].to_owned()
    } else {
        word.to_owned()
    }
}

fn plural(word: &str) -> String {
    let vowel = |c: char| "aeiou".contains(c);
    if let Some(stem) = word.strip_suffix('y')
        && !stem.ends_with(vowel)
    {
        return format!("{stem}ies");
    }
    if ["s", "sh", "ch", "x", "z"]
        .iter()
        .any(|s| word.ends_with(s))
    {
        return format!("{word}es");
    }
    format!("{word}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_and_transforms() {
        let parameters = HashMap::from([
            ("item".to_owned(), "Pet".to_owned()),
            ("resourcePathName".to_owned(), "pets".to_owned()),
        ]);
        assert_eq!(replace("<<item>>[]", &parameters), "Pet[]");
        assert_eq!(
            replace("<<resourcePathName | !singularize>>Id", &parameters),
            "petId"
        );
        assert_eq!(
            replace("<<item | !pluralize | !uppercase>>", &parameters),
            "PETS"
        );
        assert_eq!(
            replace("<<unknown>> <<item", &parameters),
            "<<unknown>> <<item"
        );
        for (value, transform, expected) in [
            ("pets", "singularize", "pet"),
            ("categories", "singularize", "category"),
            ("boxes", "singularize", "box"),
            ("class", "singularize", "class"),
            ("category", "pluralize", "categories"),
            ("box", "pluralize", "boxes"),
            ("day", "pluralize", "days"),
            ("user_id", "lowercamelcase", "userId"),
            ("userId", "uppercamelcase", "UserId"),
            ("UserId", "lowerunderscorecase", "user_id"),
            ("userId", "upperhyphencase", "USER-ID"),
        ] {
            assert_eq!(
                transformed(value, transform),
                expected,
                "{value} | !{transform}"
            );
        }
        assert_eq!(resource_path_name("/pets/{petId}"), "pets");
        assert_eq!(resource_path_name("/stores/{id}/pets"), "pets");
    }
}
