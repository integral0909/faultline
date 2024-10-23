//! Libraries: `uses: {lib: path/to/library.raml}` makes a library's types,
//! traits and resource types available as `lib.Name`.

use faultline_doc::{Node, Value};
use faultline_source::{Diagnostic, Span};

use crate::codes;
use crate::load::{Document, RamlLoader};
use crate::templates::Templates;
use crate::types::Scope;

/// Libraries use other libraries at most this deep, which also ends
/// libraries that use each other.
const MAX_DEPTH: usize = 16;

/// The types and templates visible from `document`: its own, and its
/// libraries' under their names, recursively.
pub(crate) fn scope_of(
    raml: &mut RamlLoader<'_>,
    document: &Document,
    prefix: &str,
) -> (Scope, Templates) {
    let mut scope = Scope::of(&document.root, prefix);
    let mut templates = Templates::of(&document.root);
    add_uses(raml, document, prefix, 0, &mut scope, &mut templates);
    (scope, templates)
}

fn add_uses(
    raml: &mut RamlLoader<'_>,
    document: &Document,
    prefix: &str,
    depth: usize,
    scope: &mut Scope,
    templates: &mut Templates,
) {
    let Some(uses) = document.root.get("uses").and_then(Node::as_object) else {
        return;
    };
    for (name, path) in uses.iter() {
        let Value::String(text) = &path.value else {
            error(raml, "a library is named by its path".into(), path.span);
            continue;
        };
        if depth >= MAX_DEPTH {
            error(
                raml,
                format!("libraries use each other more than {MAX_DEPTH} deep"),
                path.span,
            );
            continue;
        }
        let Some(target) = raml.resolve(text, &document.uri, path.span) else {
            continue;
        };
        let Some(library) = raml.document(&target, Some(path.span)) else {
            continue;
        };
        if library.kind != "Library" {
            error(
                raml,
                format!("{text} is not a RAML library (`#%RAML 1.0 Library`)"),
                path.span,
            );
            continue;
        }
        let library_prefix = format!("{prefix}{name}.");
        let mut library_scope = Scope::of(&library.root, &library_prefix);
        let mut library_templates = Templates::of(&library.root);
        add_uses(
            raml,
            &library,
            &library_prefix,
            depth + 1,
            &mut library_scope,
            &mut library_templates,
        );
        library_templates.qualify(&format!("{name}."));
        scope.libraries.insert(name.to_owned(), library_scope);
        templates
            .libraries
            .insert(name.to_owned(), library_templates);
    }
}

fn error(raml: &mut RamlLoader<'_>, message: String, span: Span) {
    raml.diagnostics.push(
        Diagnostic::error(message)
            .with_code(codes::UNRESOLVED)
            .with_primary(span, ""),
    );
}
