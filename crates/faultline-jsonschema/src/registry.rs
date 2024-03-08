//! Schema resources, identifiers and reference resolution.
//!
//! JSON Schema identifies schemas by URI. A *resource* is a schema with an
//! identifier (`$id`, or the document it lives in); references are resolved
//! against the identifier of the innermost enclosing resource, and fragments
//! are either JSON Pointers relative to that resource or anchor names declared
//! inside it.
//!
//! The registry learns identifiers by scanning schemas — visiting exactly the
//! subschema positions of each keyword — and loads further documents through
//! the [`DocumentStore`] when a reference names one it has not seen.

use std::collections::HashMap;
use std::sync::Arc;

use faultline_doc::{
    DocumentStore, JsonPointer, Node, StoreError, StoredDocument, UriRef, Value, loader::LoadError,
};
use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::codes;
use crate::dialect::Dialect;
use crate::keywords::{for_each_subschema, is_schema_value};

/// A position in a loaded document.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Location {
    /// Normalized document URI, without fragment.
    pub document: UriRef,
    pub pointer: JsonPointer,
}

impl Location {
    pub fn new(document: UriRef, pointer: JsonPointer) -> Self {
        Self { document, pointer }
    }

    #[must_use]
    pub fn join<I, S>(&self, tokens: I) -> Location
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut pointer = self.pointer.clone();
        for token in tokens {
            pointer.push(token);
        }
        Location::new(self.document.clone(), pointer)
    }
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.document, self.pointer.to_uri_fragment())
    }
}

/// What the registry knows about a scanned schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaInfo {
    /// The base URI references in this schema resolve against.
    pub base: UriRef,
    pub dialect: Dialect,
    /// Identifier of the innermost resource containing the schema.
    pub resource: UriRef,
}

#[derive(Clone, Debug)]
pub(crate) struct Resource {
    pub(crate) root: Location,
    pub(crate) anchors: HashMap<String, Location>,
    pub(crate) dynamic_anchors: HashMap<String, Location>,
    /// `$recursiveAnchor: true` on the resource root (2019-09).
    pub(crate) recursive_anchor: bool,
    /// Span of the identifier, for duplicate reporting.
    declared_at: Option<Span>,
}

impl Resource {
    fn new(root: Location, declared_at: Option<Span>) -> Self {
        Self {
            root,
            anchors: HashMap::new(),
            dynamic_anchors: HashMap::new(),
            recursive_anchor: false,
            declared_at,
        }
    }
}

/// Why a reference could not be resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResolveError {
    InvalidUri(String),
    OutsideRoot,
    Load(LoadError),
    /// The target document failed to parse (diagnosed separately).
    Parse,
    InvalidPointer(String),
    UnknownAnchor(String),
    Missing,
    NotASchema,
}

#[derive(Default)]
pub(crate) struct Registry {
    documents: HashMap<UriRef, Arc<StoredDocument>>,
    resources: HashMap<UriRef, Resource>,
    schemas: HashMap<Location, SchemaInfo>,
    /// Scan order of schema locations per document, for deterministic
    /// iteration over definitions.
    scan_order: Vec<Location>,
    /// Number of resources declaring each dynamic anchor, maintained as
    /// anchors are declared so memoization keys stay cheap to compute.
    dynamic_anchor_counts: HashMap<String, usize>,
    recursive_anchors: usize,
}

impl Registry {
    /// Registers a loaded document as a resource under its own URI.
    pub(crate) fn add_document(&mut self, document: Arc<StoredDocument>) {
        let uri = document.uri.clone();
        if self.documents.contains_key(&uri) {
            return;
        }
        self.resources.entry(uri.clone()).or_insert_with(|| {
            Resource::new(Location::new(uri.clone(), JsonPointer::root()), None)
        });
        self.documents.insert(uri, document);
    }

    pub(crate) fn document(&self, uri: &UriRef) -> Option<Arc<StoredDocument>> {
        self.documents.get(uri).cloned()
    }

    pub(crate) fn resource(&self, uri: &UriRef) -> Option<&Resource> {
        self.resources.get(&uri.normalized())
    }

    pub(crate) fn info(&self, location: &Location) -> Option<&SchemaInfo> {
        self.schemas.get(location)
    }

    /// Schema locations in the order they were first scanned.
    pub(crate) fn scanned(&self) -> &[Location] {
        &self.scan_order
    }

    /// Scans the schema at `location` and everything below it, recording
    /// identifiers, anchors and the base URI of every subschema.
    pub(crate) fn scan(
        &mut self,
        location: Location,
        base: UriRef,
        dialect: Dialect,
        resource: UriRef,
        diagnostics: &mut Diagnostics,
    ) {
        let Some(document) = self.documents.get(&location.document).cloned() else {
            return;
        };
        let mut stack = vec![(location, base, dialect, resource)];
        while let Some((location, mut base, mut dialect, mut resource)) = stack.pop() {
            if self.schemas.contains_key(&location) {
                continue;
            }
            let Some(node) = document.document.root.pointer(&location.pointer) else {
                continue;
            };
            let Value::Object(obj) = &node.value else {
                if is_schema_value(node, dialect) {
                    self.record(location, base, dialect, resource);
                }
                continue;
            };

            // `$schema` is honoured at resource roots. Either spelling of the
            // identifier counts, since `$schema` may switch to a dialect that
            // uses the other one.
            let is_resource_root =
                location.pointer.is_root() || obj.contains_key("$id") || obj.contains_key("id");
            if is_resource_root && let Some(meta) = obj.get("$schema") {
                match meta.as_str().and_then(Dialect::from_meta_schema) {
                    Some(d) => dialect = d,
                    None => diagnostics.push(
                        Diagnostic::warning("unknown $schema dialect")
                            .with_code(codes::UNKNOWN_DIALECT)
                            .with_primary(meta.span, format!("treated as {dialect}"))
                            .with_help("use a draft-04 to 2020-12 meta-schema URI"),
                    ),
                }
            }
            // The id keyword depends on the (possibly just updated) dialect.
            if let Some(id_node) = obj.get(dialect.id_keyword())
                && let Some((new_base, anchor)) = declare_id(id_node, &base, dialect, diagnostics)
            {
                if let Some(name) = anchor {
                    self.declare_anchor(
                        &resource,
                        &name,
                        &location,
                        false,
                        id_node.span,
                        diagnostics,
                    );
                } else {
                    self.declare_resource(&new_base, &location, id_node.span, diagnostics);
                    base = new_base.clone();
                    resource = new_base;
                }
            }
            if dialect.has_anchor_keyword()
                && let Some(anchor) = obj.get("$anchor")
            {
                self.declare_anchor_node(&resource, anchor, &location, false, diagnostics);
            }
            if dialect.has_dynamic_ref()
                && let Some(anchor) = obj.get("$dynamicAnchor")
            {
                self.declare_anchor_node(&resource, anchor, &location, true, diagnostics);
            }
            if dialect.has_recursive_ref()
                && obj.get("$recursiveAnchor").and_then(Node::as_bool) == Some(true)
                && let Some(r) = self.resources.get_mut(&resource)
                && r.root == location
            {
                if !r.recursive_anchor {
                    self.recursive_anchors += 1;
                }
                r.recursive_anchor = true;
            }

            let mut children = Vec::new();
            for_each_subschema(obj, dialect, |tokens, _| children.push(tokens));
            // Reverse so the stack pops children in document order.
            for tokens in children.into_iter().rev() {
                stack.push((
                    location.join(tokens),
                    base.clone(),
                    dialect,
                    resource.clone(),
                ));
            }
            self.record(location, base, dialect, resource);
        }
    }

    fn record(&mut self, location: Location, base: UriRef, dialect: Dialect, resource: UriRef) {
        self.scan_order.push(location.clone());
        self.schemas.insert(
            location,
            SchemaInfo {
                base,
                dialect,
                resource,
            },
        );
    }

    fn declare_resource(
        &mut self,
        uri: &UriRef,
        location: &Location,
        span: Span,
        diagnostics: &mut Diagnostics,
    ) {
        match self.resources.get(uri) {
            Some(existing) if existing.root != *location => {
                let mut diagnostic =
                    Diagnostic::error(format!("duplicate schema identifier {uri}"))
                        .with_code(codes::DUPLICATE_IDENTIFIER)
                        .with_primary(span, "declared again here");
                if let Some(first) = existing.declared_at {
                    diagnostic = diagnostic.with_secondary(first, "first declared here");
                }
                diagnostics.push(diagnostic);
            }
            Some(_) => {}
            None => {
                self.resources
                    .insert(uri.clone(), Resource::new(location.clone(), Some(span)));
            }
        }
    }

    fn declare_anchor_node(
        &mut self,
        resource: &UriRef,
        node: &Node,
        location: &Location,
        dynamic: bool,
        diagnostics: &mut Diagnostics,
    ) {
        match node.as_str() {
            Some(name) if is_anchor_name(name) => {
                self.declare_anchor(resource, name, location, dynamic, node.span, diagnostics);
            }
            _ => diagnostics.push(
                Diagnostic::error("invalid anchor name")
                    .with_code(codes::INVALID_KEYWORD)
                    .with_primary(node.span, "anchors are plain names: a letter or '_' followed by letters, digits, '-', '_', '.'"),
            ),
        }
    }

    fn declare_anchor(
        &mut self,
        resource: &UriRef,
        name: &str,
        location: &Location,
        dynamic: bool,
        span: Span,
        diagnostics: &mut Diagnostics,
    ) {
        let Some(r) = self.resources.get_mut(resource) else {
            return;
        };
        if let Some(existing) = r.anchors.get(name)
            && existing != location
        {
            diagnostics.push(
                Diagnostic::error(format!("duplicate anchor \"{name}\" in {resource}"))
                    .with_code(codes::DUPLICATE_IDENTIFIER)
                    .with_primary(span, "declared again here"),
            );
            return;
        }
        r.anchors.insert(name.to_owned(), location.clone());
        if dynamic
            && r.dynamic_anchors
                .insert(name.to_owned(), location.clone())
                .is_none()
        {
            *self
                .dynamic_anchor_counts
                .entry(name.to_owned())
                .or_default() += 1;
        }
    }

    /// Resolves `reference`, found in the schema at `from`, to a schema
    /// location, loading and scanning documents as needed.
    pub(crate) fn resolve(
        &mut self,
        store: &mut DocumentStore<'_>,
        reference: &str,
        from: &SchemaInfo,
        diagnostics: &mut Diagnostics,
    ) -> Result<Location, ResolveError> {
        let parsed =
            UriRef::parse(reference).map_err(|e| ResolveError::InvalidUri(e.to_string()))?;
        let target = resolve_against(&from.base, &parsed)?;
        let resource_uri = target.without_fragment().normalized();

        if !self.resources.contains_key(&resource_uri) {
            let document = store.get(&resource_uri).map_err(|e| match e {
                StoreError::Load(e) => ResolveError::Load(e),
                StoreError::Parse => ResolveError::Parse,
            })?;
            let doc_uri = document.uri.clone();
            self.add_document(document);
            // A loaded document inherits the referencing schema's dialect
            // unless it declares its own.
            let root = Location::new(doc_uri.clone(), JsonPointer::root());
            self.scan(root, doc_uri.clone(), from.dialect, doc_uri, diagnostics);
        }
        let resource = self
            .resources
            .get(&resource_uri)
            .ok_or(ResolveError::Missing)?;

        let location = match target.fragment() {
            None | Some("") => resource.root.clone(),
            Some(fragment) if fragment.starts_with('/') => {
                let pointer = JsonPointer::from_uri_fragment(fragment)
                    .map_err(|e| ResolveError::InvalidPointer(e.to_string()))?;
                resource.root.join(pointer.tokens().iter().cloned())
            }
            Some(name) => resource
                .anchors
                .get(name)
                .cloned()
                .ok_or_else(|| ResolveError::UnknownAnchor(name.to_owned()))?,
        };
        self.ensure_scanned(&location, from.dialect, diagnostics)?;
        Ok(location)
    }

    /// Makes sure `location` holds a schema and has been scanned. Locations
    /// that are not subschema positions of anything scanned (for example a
    /// schema inside a vendor extension) are scanned on demand, inheriting the
    /// base URI of their nearest scanned ancestor.
    pub(crate) fn ensure_scanned(
        &mut self,
        location: &Location,
        dialect: Dialect,
        diagnostics: &mut Diagnostics,
    ) -> Result<(), ResolveError> {
        if self.schemas.contains_key(location) {
            return Ok(());
        }
        let document = self
            .documents
            .get(&location.document)
            .ok_or(ResolveError::Missing)?;
        let node = document
            .document
            .root
            .pointer(&location.pointer)
            .ok_or(ResolveError::Missing)?;
        if !is_schema_value(node, dialect) {
            return Err(ResolveError::NotASchema);
        }
        let mut ancestor = location.pointer.parent();
        let inherited = loop {
            match ancestor {
                Some(pointer) => {
                    let candidate = Location::new(location.document.clone(), pointer.clone());
                    if let Some(info) = self.schemas.get(&candidate) {
                        break info.clone();
                    }
                    ancestor = pointer.parent();
                }
                None => {
                    break SchemaInfo {
                        base: location.document.clone(),
                        dialect,
                        resource: location.document.clone(),
                    };
                }
            }
        };
        self.scan(
            location.clone(),
            inherited.base,
            inherited.dialect,
            inherited.resource,
            diagnostics,
        );
        Ok(())
    }

    /// Anchor names declared as dynamic in more than one resource. Only these
    /// can resolve differently depending on the dynamic scope, so only they
    /// need to be part of memoization keys.
    pub(crate) fn rebindable_dynamic_anchors(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .dynamic_anchor_counts
            .iter()
            .filter(|&(_, &count)| count > 1)
            .map(|(name, _)| name.as_str())
            .collect();
        names.sort_unstable();
        names
    }

    pub(crate) fn recursive_anchor_count(&self) -> usize {
        self.recursive_anchors
    }
}

/// Interprets an identifier keyword. Returns the new base URI and, for a
/// fragment-only identifier before 2019-09, the anchor name it declares.
fn declare_id(
    id_node: &Node,
    base: &UriRef,
    dialect: Dialect,
    diagnostics: &mut Diagnostics,
) -> Option<(UriRef, Option<String>)> {
    let invalid = |diagnostics: &mut Diagnostics, message: &str| {
        diagnostics.push(
            Diagnostic::error(format!("invalid {}", dialect.id_keyword()))
                .with_code(codes::INVALID_URI)
                .with_primary(id_node.span, message.to_owned()),
        );
        None
    };
    let Some(text) = id_node.as_str() else {
        return invalid(diagnostics, "must be a string");
    };
    let Ok(id) = UriRef::parse(text) else {
        return invalid(diagnostics, "not a valid URI reference");
    };
    match id.fragment() {
        Some(fragment) if !fragment.is_empty() => {
            if !dialect.has_anchor_keyword() && id.is_fragment_only() {
                return Some((base.clone(), Some(fragment.to_owned())));
            }
            invalid(
                diagnostics,
                "identifiers must not have a fragment; use $anchor",
            )
        }
        _ => Some((base.resolve(&id).without_fragment().normalized(), None)),
    }
}

/// Resolves a reference against a base. For workspace (`file:`) bases the
/// resolution is strict, so references that climb out of the workspace are
/// reported rather than silently clamped to its root.
fn resolve_against(base: &UriRef, reference: &UriRef) -> Result<UriRef, ResolveError> {
    let is_file = base
        .scheme()
        .is_some_and(|s| s.eq_ignore_ascii_case("file"));
    if is_file && !reference.has_scheme() {
        base.resolve_strict(reference)
            .ok_or(ResolveError::OutsideRoot)
    } else {
        Ok(base.resolve(reference))
    }
}

/// Anchor names per JSON Schema 2019-09+: `[A-Za-z_][-A-Za-z0-9._]*`.
fn is_anchor_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}
