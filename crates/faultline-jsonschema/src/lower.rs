//! Lowering of JSON Schemas into the IR.
//!
//! Every schema location becomes one IR node, memoized per location (and per
//! dynamic scope, see below). Lowering never recurses: asking for the node of
//! a subschema or reference target reserves an id and queues the location,
//! and [`Lowerer::drain`] builds queued schemas one at a time. This bounds
//! stack use regardless of how long reference chains are, and makes recursive
//! schemas fall out naturally — a schema that refers to itself simply receives
//! its own reserved id.
//!
//! ## Dynamic scope
//!
//! `$dynamicRef` (2020-12) and `$recursiveRef` (2019-09) resolve differently
//! depending on which schema resources were entered on the way to them. The
//! same location can therefore denote different types, so memoization keys
//! include the *bindings* the dynamic scope implies for every anchor that is
//! actually declared by more than one resource. Documents without such anchors
//! get empty bindings and one node per location.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use std::sync::Arc;

use faultline_doc::{
    DocumentStore, JsonPointer, Node, StoreError, StoredDocument, UriRef, Value, loader::LoadError,
};
use faultline_ir::{Api, QualName, TypeId, TypeKind, TypeNode};
use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::codes;
use crate::dialect::Dialect;
use crate::registry::{Location, Registry, ResolveError, SchemaInfo};

#[derive(Clone, Debug)]
pub struct Options {
    /// Dialect for schemas that do not declare one with `$schema`.
    pub default_dialect: Dialect,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            default_dialect: Dialect::Draft2020_12,
        }
    }
}

/// The dynamic scope: resources entered on the way to a schema, innermost
/// first. A persistent list, because every child shares its parent's scope.
#[derive(Debug)]
pub(crate) struct ScopeNode {
    pub(crate) resource: UriRef,
    pub(crate) parent: Option<Rc<ScopeNode>>,
}

pub(crate) type Scope = Option<Rc<ScopeNode>>;

/// Resources in the scope, outermost first.
pub(crate) fn outermost_first(scope: &Scope) -> Vec<UriRef> {
    let mut resources = Vec::new();
    let mut cursor = scope.as_ref();
    while let Some(node) = cursor {
        resources.push(node.resource.clone());
        cursor = node.parent.as_ref();
    }
    resources.reverse();
    resources
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    location: Location,
    /// `(anchor, resource)` pairs; the empty anchor stands for
    /// `$recursiveAnchor`.
    bindings: Vec<(String, UriRef)>,
}

struct Job {
    location: Location,
    scope: Scope,
    id: TypeId,
    name: Option<QualName>,
}

/// Lowers schema locations into the type arena of an [`Api`].
///
/// Front ends that embed JSON Schema (OpenAPI) use this directly: register
/// their documents, [`lower`](Self::lower) each schema position they find, and
/// call [`finish`](Self::finish) for the diagnostics.
pub struct Lowerer<'a, 'l> {
    pub(crate) api: &'a mut Api,
    pub(crate) store: &'a mut DocumentStore<'l>,
    pub(crate) registry: Registry,
    pub(crate) options: Options,
    memo: HashMap<Key, TypeId>,
    queue: VecDeque<Job>,
    names: HashMap<String, Location>,
    pub(crate) diagnostics: Diagnostics,
    any: Option<TypeId>,
    never: Option<TypeId>,
}

impl std::fmt::Debug for Lowerer<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lowerer")
            .field("memoized", &self.memo.len())
            .field("queued", &self.queue.len())
            .finish_non_exhaustive()
    }
}

impl<'a, 'l> Lowerer<'a, 'l> {
    pub fn new(api: &'a mut Api, store: &'a mut DocumentStore<'l>, options: Options) -> Self {
        Self {
            api,
            store,
            registry: Registry::default(),
            options,
            memo: HashMap::new(),
            queue: VecDeque::new(),
            names: HashMap::new(),
            diagnostics: Diagnostics::new(),
            any: None,
            never: None,
        }
    }

    /// Loads the document at `uri` and scans its root as a schema. Returns
    /// the root location, or `None` if the document could not be loaded (a
    /// diagnostic has been recorded).
    pub fn load_schema_document(&mut self, uri: &UriRef) -> Option<Location> {
        let document = match self.store.get(uri) {
            Ok(document) => document,
            Err(faultline_doc::StoreError::Load(e)) => {
                self.diagnostics.push(
                    Diagnostic::error(format!("cannot load {uri}: {e}")).with_code(load_code(&e)),
                );
                return None;
            }
            Err(faultline_doc::StoreError::Parse) => return None,
        };
        let root = Location::new(document.uri.clone(), JsonPointer::root());
        self.registry.add_document(document);
        let uri = root.document.clone();
        self.registry.scan(
            root.clone(),
            uri.clone(),
            self.options.default_dialect,
            uri,
            &mut self.diagnostics,
        );
        Some(root)
    }

    /// Loads the document at `uri` and registers it for reference
    /// resolution without treating its root as a schema, as for an OpenAPI
    /// description whose schemas sit at known positions inside it.
    pub fn document(&mut self, uri: &UriRef) -> Result<Arc<StoredDocument>, StoreError> {
        let document = self.store.get(uri)?;
        self.registry.add_document(Arc::clone(&document));
        Ok(document)
    }

    /// The node for the schema at `location` in a registered document,
    /// scanning it first with `dialect` if it was not reached as part of
    /// another schema. Returns `None` (with a diagnostic) if the location does
    /// not hold a schema.
    pub fn schema_at(&mut self, location: &Location, dialect: Dialect) -> Option<TypeId> {
        match self
            .registry
            .ensure_scanned(location, dialect, &mut self.diagnostics)
        {
            Ok(()) => Some(self.lower(location)),
            Err(error) => {
                let span = self
                    .registry
                    .document(&location.document)
                    .and_then(|d| d.document.root.pointer(&location.pointer).map(|n| n.span));
                if let Some(span) = span {
                    self.diagnostics.push(resolve_diagnostic(
                        &location.pointer.to_string(),
                        &error,
                        span,
                    ));
                }
                None
            }
        }
    }

    /// The API being built, for front ends that add surfaces around the
    /// schemas they lower.
    pub fn api_mut(&mut self) -> &mut Api {
        self.api
    }

    /// The node for the schema at `location`, lowering it (and everything it
    /// reaches) if necessary.
    pub fn lower(&mut self, location: &Location) -> TypeId {
        let id = self.target(location, &None);
        self.drain();
        id
    }

    /// Lowers every definition (`$defs`, `definitions`) declared in
    /// `document`, in document order, so that reusable schemas are part of
    /// the contract even when nothing in the document refers to them.
    pub fn lower_definitions(&mut self, document: &UriRef) {
        let definitions: Vec<Location> = self
            .registry
            .scanned()
            .iter()
            .filter(|l| &l.document == document && is_definition(&l.pointer))
            .cloned()
            .collect();
        for location in definitions {
            self.target(&location, &None);
        }
        self.drain();
    }

    /// Validates the lowered graph and returns every diagnostic raised.
    pub fn finish(mut self) -> Diagnostics {
        self.drain();
        for error in faultline_ir::validate(self.api) {
            let diagnostic = match error {
                faultline_ir::IrError::UnguardedCycle { cycle } => {
                    let spans: Vec<Span> = cycle
                        .iter()
                        .filter_map(|id| self.api.types.get(*id).origin)
                        .collect();
                    let mut d = Diagnostic::error("schema is defined in terms of itself")
                        .with_code(codes::UNGUARDED_RECURSION)
                        .with_note("a recursive schema must pass through a property or array item before referring to itself; otherwise it describes no values");
                    if let Some((first, rest)) = spans.split_first() {
                        d = d.with_primary(*first, "this schema refers back to itself");
                        for span in rest {
                            d = d.with_secondary(*span, "through this schema");
                        }
                    }
                    d
                }
                other => {
                    Diagnostic::error(format!("internal error while lowering schemas: {other}"))
                        .with_code(codes::INTERNAL)
                }
            };
            self.diagnostics.push(diagnostic);
        }
        let mut diagnostics = self.store.take_diagnostics();
        diagnostics.extend(self.diagnostics);
        diagnostics.sort();
        diagnostics
    }

    /// Returns the id for `location` under `scope`, reserving and queueing it
    /// on first use.
    pub(crate) fn target(&mut self, location: &Location, scope: &Scope) -> TypeId {
        let Some(info) = self.registry.info(location).cloned() else {
            // Callers resolve or scan locations first; an unscanned location
            // here means the pointer did not name a schema.
            return self.any();
        };
        let scope = enter(scope, &info.resource);
        let key = Key {
            location: location.clone(),
            bindings: self.bindings(&scope),
        };
        if let Some(&id) = self.memo.get(&key) {
            return id;
        }
        let id = self.api.types.reserve();
        let name = self.name_for(location, &info, &key);
        if let Some(name) = &name {
            self.api.named.insert(name.clone(), id);
        }
        self.memo.insert(key, id);
        self.queue.push_back(Job {
            location: location.clone(),
            scope,
            id,
            name,
        });
        id
    }

    fn drain(&mut self) {
        while let Some(job) = self.queue.pop_front() {
            let mut node = self.build(&job.location, &job.scope);
            node.name = job.name;
            self.api.types.define(job.id, node);
        }
    }

    fn build(&mut self, location: &Location, scope: &Scope) -> TypeNode {
        let info = self
            .registry
            .info(location)
            .cloned()
            .expect("queued locations are scanned");
        let document = self
            .registry
            .document(&location.document)
            .expect("scanned locations belong to loaded documents");
        let node = document
            .document
            .root
            .pointer(&location.pointer)
            .expect("scanned locations exist");
        match &node.value {
            Value::Bool(false) => TypeNode::new(TypeKind::Never).with_origin(node.span),
            Value::Object(_) => {
                let ctx = SchemaCtx {
                    location: location.clone(),
                    info,
                    scope: scope.clone(),
                    node,
                };
                self.build_schema(&ctx)
            }
            // `true`, and anything that is not a schema (already diagnosed).
            _ => TypeNode::new(TypeKind::Any).with_origin(node.span),
        }
    }

    /// The type for a subschema at `tokens` below the schema in `ctx`.
    /// Boolean subschemas map to shared `Any`/`Never` nodes; object
    /// subschemas are memoized per location.
    pub(crate) fn child(&mut self, ctx: &SchemaCtx<'_>, tokens: &[&str]) -> TypeId {
        let location = ctx.location.join(tokens.iter().copied());
        let Some(node) = ctx.node_at(tokens) else {
            return self.any();
        };
        match node.value {
            // `true`/`false` carry no metadata, so sharing is safe, and draft-04
            // allows boolean `additionalProperties`/`additionalItems` even though
            // it has no boolean schemas in general.
            Value::Bool(true) => self.any(),
            Value::Bool(false) => self.never(),
            _ => {
                if self
                    .registry
                    .ensure_scanned(&location, ctx.info.dialect, &mut self.diagnostics)
                    .is_err()
                {
                    return self.any();
                }
                self.target(&location, &ctx.scope)
            }
        }
    }

    /// Resolves a reference and returns the id of its target, reporting
    /// failures at `span`.
    pub(crate) fn reference(
        &mut self,
        ctx: &SchemaCtx<'_>,
        reference: &str,
        span: Span,
    ) -> Option<Location> {
        match self
            .registry
            .resolve(self.store, reference, &ctx.info, &mut self.diagnostics)
        {
            Ok(location) => Some(location),
            Err(error) => {
                self.diagnostics
                    .push(resolve_diagnostic(reference, &error, span));
                None
            }
        }
    }

    pub(crate) fn any(&mut self) -> TypeId {
        *self
            .any
            .get_or_insert_with(|| self.api.types.alloc_kind(TypeKind::Any))
    }

    pub(crate) fn never(&mut self) -> TypeId {
        *self
            .never
            .get_or_insert_with(|| self.api.types.alloc_kind(TypeKind::Never))
    }

    pub(crate) fn alloc(&mut self, kind: TypeKind) -> TypeId {
        self.api.types.alloc_kind(kind)
    }

    /// Bindings of rebindable dynamic anchors under `scope`: for each, the
    /// outermost resource in scope that declares it.
    fn bindings(&self, scope: &Scope) -> Vec<(String, UriRef)> {
        let rebindable = self.registry.rebindable_dynamic_anchors();
        let recursive = self.registry.recursive_anchor_count() > 1;
        if rebindable.is_empty() && !recursive {
            return Vec::new();
        }
        let resources = outermost_first(scope);
        let mut bindings = Vec::new();
        for name in rebindable {
            if let Some(resource) = resources.iter().find(|r| {
                self.registry
                    .resource(r)
                    .is_some_and(|res| res.dynamic_anchors.contains_key(name))
            }) {
                bindings.push((name.to_owned(), resource.clone()));
            }
        }
        if recursive
            && let Some(resource) = resources.iter().find(|r| {
                self.registry
                    .resource(r)
                    .is_some_and(|res| res.recursive_anchor)
            })
        {
            bindings.push((String::new(), resource.clone()));
        }
        bindings
    }

    /// The IR name for a location, if it is a named type: a definition, a
    /// document root, or the root of an identified resource.
    fn name_for(&mut self, location: &Location, info: &SchemaInfo, key: &Key) -> Option<QualName> {
        let base = if is_definition(&location.pointer) {
            location.pointer.tokens().last().cloned()?
        } else if location.pointer.is_root() {
            file_stem(&location.document)
        } else if self
            .registry
            .resource(&info.resource)
            .is_some_and(|r| r.root == *location)
        {
            file_stem(&info.resource)
        } else {
            return None;
        };

        let mut candidates = vec![base.clone()];
        let stem = file_stem(&location.document);
        if stem != base {
            candidates.push(format!("{stem}.{base}"));
        }
        candidates.push(location.to_string());
        for candidate in candidates {
            match self.names.get(&candidate) {
                None => {
                    self.names.insert(candidate.clone(), location.clone());
                    return Some(QualName::new(candidate));
                }
                // Taken by this very location under another dynamic scope:
                // qualifying by file would not help, specialise instead.
                Some(existing) if existing == location => break,
                Some(_) => {}
            }
        }
        // The same location specialised for another dynamic scope: name it
        // after the resources its dynamic references are bound to.
        let bound: Vec<String> = key.bindings.iter().map(|(_, r)| file_stem(r)).collect();
        let specialised = format!("{base}@{}", bound.join(","));
        let mut candidate = specialised.clone();
        let mut n = 2;
        while self.names.contains_key(&candidate) {
            candidate = format!("{specialised}#{n}");
            n += 1;
        }
        self.names.insert(candidate.clone(), location.clone());
        Some(QualName::new(candidate))
    }
}

/// A schema object being lowered, with everything needed to resolve its
/// subschemas.
pub(crate) struct SchemaCtx<'n> {
    pub(crate) location: Location,
    pub(crate) info: SchemaInfo,
    pub(crate) scope: Scope,
    pub(crate) node: &'n Node,
}

impl<'n> SchemaCtx<'n> {
    pub(crate) fn get(&self, keyword: &str) -> Option<&'n Node> {
        self.node.get(keyword)
    }

    pub(crate) fn node_at(&self, tokens: &[&str]) -> Option<&'n Node> {
        tokens
            .iter()
            .try_fold(self.node, |node, token| match &node.value {
                Value::Object(obj) => obj.get(token),
                Value::Array(items) => token.parse::<usize>().ok().and_then(|i| items.get(i)),
                _ => None,
            })
    }

    pub(crate) fn dialect(&self) -> Dialect {
        self.info.dialect
    }
}

fn enter(scope: &Scope, resource: &UriRef) -> Scope {
    match scope {
        Some(top) if top.resource == *resource => scope.clone(),
        _ => Some(Rc::new(ScopeNode {
            resource: resource.clone(),
            parent: scope.clone(),
        })),
    }
}

/// Whether a pointer names an entry of a definitions container: `$defs` or
/// `definitions` anywhere, or OpenAPI's `components/schemas` at the root.
pub(crate) fn is_definition(pointer: &JsonPointer) -> bool {
    match pointer.tokens() {
        [components, schemas, _] if components == "components" && schemas == "schemas" => true,
        [.., container, _] => container == "$defs" || container == "definitions",
        _ => false,
    }
}

/// The last path segment of a URI without its extension, e.g. `pet` for
/// `file:///schemas/pet.json`.
fn file_stem(uri: &UriRef) -> String {
    let segment = uri
        .path()
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("schema");
    let decoded = faultline_doc::uri::percent_decode(segment).unwrap_or_else(|| segment.to_owned());
    match decoded.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_owned(),
        _ => decoded,
    }
}

fn load_code(error: &LoadError) -> faultline_source::DiagnosticCode {
    match error {
        LoadError::OutsideRoot => codes::REFERENCE_OUTSIDE_ROOT,
        LoadError::RemoteDisabled { .. } => codes::REMOTE_REFERENCE,
        _ => codes::LOAD_FAILED,
    }
}

fn resolve_diagnostic(reference: &str, error: &ResolveError, span: Span) -> Diagnostic {
    let (code, label, help) = match error {
        ResolveError::InvalidUri(e) => (
            codes::INVALID_URI,
            format!("not a valid URI reference: {e}"),
            None,
        ),
        ResolveError::OutsideRoot => (
            codes::REFERENCE_OUTSIDE_ROOT,
            "points outside the workspace root".to_owned(),
            Some("references may only reach files inside the workspace"),
        ),
        ResolveError::Load(e) => (load_code(e), e.to_string(), None),
        ResolveError::Parse => (
            codes::LOAD_FAILED,
            "the referenced document does not parse".to_owned(),
            None,
        ),
        ResolveError::InvalidPointer(e) => (codes::UNRESOLVED_REFERENCE, e.clone(), None),
        ResolveError::UnknownAnchor(name) => (
            codes::UNRESOLVED_REFERENCE,
            format!("no anchor named \"{name}\" in the target resource"),
            None,
        ),
        ResolveError::Missing => (
            codes::UNRESOLVED_REFERENCE,
            "nothing exists at this location".to_owned(),
            None,
        ),
        ResolveError::NotASchema => (
            codes::NOT_A_SCHEMA,
            "the target is not a schema".to_owned(),
            None,
        ),
    };
    let mut diagnostic = Diagnostic::error(format!("cannot resolve reference \"{reference}\""))
        .with_code(code)
        .with_primary(span, label);
    if let Some(help) = help {
        diagnostic = diagnostic.with_help(help);
    }
    diagnostic
}

/// The outermost resource in `scope` satisfying `predicate`.
pub(crate) fn outermost_with(
    registry: &Registry,
    scope: &Scope,
    predicate: impl Fn(&crate::registry::Resource) -> bool,
) -> Option<UriRef> {
    outermost_first(scope)
        .into_iter()
        .find(|r| registry.resource(r).is_some_and(&predicate))
}
