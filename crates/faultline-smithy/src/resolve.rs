//! Resolving a Smithy model's names into absolute shape ids.
//!
//! A relative shape id resolves through the file's `use` statements, then
//! the file's namespace, then the prelude (`smithy.api`). Trait ids resolve
//! the same way, except that an unknown trait is taken to be a prelude
//! trait. Resolution also flattens the model:
//!
//! * inline operation inputs and outputs become shapes of their own
//!   (`GetCityInput`);
//! * a shape's mixins contribute their members, first and in order, and
//!   their traits, except `@mixin` itself;
//! * a member with an elided target (`$cityId`) takes it from a mixin's
//!   member of the same name, or from the identifiers and properties of
//!   the resource named with `for`;
//! * `apply` statements add their traits to shapes and members.

use std::collections::{HashMap, HashSet};

use faultline_source::{Diagnostic, DiagnosticCode, Diagnostics, Span};

use crate::ast::{IoShape, Name, Node, NodeValue, Shape, ShapeKind, Trait};
use crate::codes;
use crate::load::SmithyFile;

/// The prelude's namespace.
pub const PRELUDE: &str = "smithy.api";

/// The shapes the prelude defines.
pub const PRELUDE_SHAPES: [&str; 21] = [
    "String",
    "Blob",
    "BigInteger",
    "BigDecimal",
    "Timestamp",
    "Document",
    "Boolean",
    "PrimitiveBoolean",
    "Byte",
    "PrimitiveByte",
    "Short",
    "PrimitiveShort",
    "Integer",
    "PrimitiveInteger",
    "Long",
    "PrimitiveLong",
    "Float",
    "PrimitiveFloat",
    "Double",
    "PrimitiveDouble",
    "Unit",
];

/// The id of a prelude trait or shape.
pub fn prelude(name: &str) -> String {
    format!("{PRELUDE}#{name}")
}

/// A trait with its absolute id.
#[derive(Clone, Debug, PartialEq)]
pub struct RTrait {
    pub id: String,
    pub value: Node,
}

#[derive(Clone, Debug)]
pub struct RMember {
    pub name: String,
    /// The absolute id of the target; `smithy.api#Unit` for enum members.
    pub target: String,
    pub traits: Vec<RTrait>,
    pub default: Option<Node>,
    pub doc: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct ROperation {
    pub input: Option<String>,
    pub output: Option<String>,
    pub errors: Vec<String>,
}

/// The shapes a service or resource refers to.
#[derive(Clone, Debug, Default)]
pub struct Bindings {
    /// A service's operations; a resource's operations, collection
    /// operations and lifecycle operations.
    pub operations: Vec<String>,
    pub resources: Vec<String>,
    /// A service's common errors.
    pub errors: Vec<String>,
    /// A resource's identifiers and properties: name and target.
    pub identifiers: Vec<(String, String)>,
    pub properties: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct RShape {
    pub id: String,
    pub name: String,
    pub kind: ShapeKind,
    pub traits: Vec<RTrait>,
    pub doc: Option<String>,
    pub members: Vec<RMember>,
    pub operation: Option<ROperation>,
    pub bindings: Bindings,
    /// A service's `version`.
    pub version: Option<String>,
    pub span: Span,
}

impl RShape {
    pub fn get_trait(&self, id: &str) -> Option<&RTrait> {
        self.traits.iter().find(|t| t.id == id)
    }

    pub fn has_trait(&self, name: &str) -> bool {
        self.get_trait(&prelude(name)).is_some()
    }
}

impl RMember {
    pub fn get_trait(&self, id: &str) -> Option<&RTrait> {
        self.traits.iter().find(|t| t.id == id)
    }

    pub fn has_trait(&self, name: &str) -> bool {
        self.get_trait(&prelude(name)).is_some()
    }
}

/// A resolved model.
#[derive(Debug, Default)]
pub struct Resolved {
    pub shapes: Vec<RShape>,
    pub by_id: HashMap<String, usize>,
    pub diagnostics: Diagnostics,
}

impl Resolved {
    pub fn get(&self, id: &str) -> Option<&RShape> {
        self.by_id.get(id).map(|&i| &self.shapes[i])
    }
}

/// Resolves the shapes of `files` into one model.
pub fn resolve(files: &[SmithyFile]) -> Resolved {
    let mut resolver = Resolver {
        files,
        declared: Vec::new(),
        table: HashMap::new(),
        diagnostics: Diagnostics::new(),
    };
    resolver.declare();
    let mut drafts: Vec<Draft> = (0..resolver.declared.len())
        .map(|i| resolver.draft(i))
        .collect();
    // Before mixins, so that shapes using a mixin see traits applied to it.
    resolver.apply(&mut drafts);
    let shapes = resolver.expand(&drafts);
    let by_id = shapes
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), i))
        .collect();
    let mut out = Resolved {
        shapes,
        by_id,
        diagnostics: resolver.diagnostics,
    };
    check_kinds(&mut out);
    out
}

/// A shape as declared, before mixins: its own members, those with elided
/// targets marked.
struct Draft {
    shape: RShape,
    elided: Vec<bool>,
    mixins: Vec<(String, Span)>,
    for_resource: Option<String>,
}

struct Resolver<'a> {
    files: &'a [SmithyFile],
    /// Every shape with the file it is in, inline inputs and outputs
    /// included.
    declared: Vec<(usize, Shape, String)>,
    table: HashMap<String, usize>,
    diagnostics: Diagnostics,
}

impl Resolver<'_> {
    fn error(&mut self, code: DiagnosticCode, message: String, span: Span) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_primary(span, ""),
        );
    }

    fn declare(&mut self) {
        for (file, smithy) in self.files.iter().enumerate() {
            let model = &smithy.model;
            let Some(namespace) = model.namespace.as_ref().map(|n| n.text.clone()) else {
                if let Some(first) = model.shapes.first() {
                    self.error(
                        codes::SYNTAX,
                        "a file with shapes declares a `namespace`".into(),
                        first.name.span,
                    );
                }
                continue;
            };
            for shape in &model.shapes {
                self.add(file, &namespace, shape.clone());
                if let Some(body) = &shape.operation {
                    for io in [&body.input, &body.output].into_iter().flatten() {
                        if let IoShape::Inline(inline) = io {
                            self.add(file, &namespace, (**inline).clone());
                        }
                    }
                }
            }
        }
    }

    fn add(&mut self, file: usize, namespace: &str, shape: Shape) {
        let id = format!("{namespace}#{}", shape.name.text);
        if self.table.contains_key(&id) {
            self.error(
                codes::DUPLICATE,
                format!("{id} is defined twice"),
                shape.name.span,
            );
            return;
        }
        self.table.insert(id.clone(), self.declared.len());
        self.declared.push((file, shape, id));
    }

    /// The absolute id a shape id written in `file` refers to.
    fn lookup(&self, file: usize, text: &str) -> Option<String> {
        if let Some((namespace, name)) = text.split_once('#') {
            let known = self.table.contains_key(text)
                || (namespace == PRELUDE && PRELUDE_SHAPES.contains(&name));
            return known.then(|| text.to_owned());
        }
        let model = &self.files[file].model;
        for used in &model.uses {
            if used.text.rsplit_once('#').is_some_and(|(_, n)| n == text) {
                return Some(used.text.clone());
            }
        }
        if let Some(namespace) = &model.namespace {
            let local = format!("{}#{text}", namespace.text);
            if self.table.contains_key(&local) {
                return Some(local);
            }
        }
        PRELUDE_SHAPES.contains(&text).then(|| prelude(text))
    }

    fn target(&mut self, file: usize, name: &Name) -> String {
        self.lookup(file, &name.text).unwrap_or_else(|| {
            self.error(
                codes::UNRESOLVED,
                format!("shape {} is not defined", name.text),
                name.span,
            );
            prelude("Document")
        })
    }

    /// The absolute id of a trait written in `file`: unknown relative ids
    /// are prelude traits.
    fn trait_id(&self, file: usize, text: &str) -> String {
        if text.contains('#') {
            return text.to_owned();
        }
        let model = &self.files[file].model;
        for used in &model.uses {
            if used.text.rsplit_once('#').is_some_and(|(_, n)| n == text) {
                return used.text.clone();
            }
        }
        if let Some(namespace) = &model.namespace {
            let local = format!("{}#{text}", namespace.text);
            if self.table.contains_key(&local) {
                return local;
            }
        }
        prelude(text)
    }

    fn traits(&self, file: usize, traits: &[Trait]) -> Vec<RTrait> {
        traits
            .iter()
            .map(|t| RTrait {
                id: self.trait_id(file, &t.id.text),
                value: t.value.clone(),
            })
            .collect()
    }

    /// The shape ids listed in a property of a service or resource.
    fn ids(&mut self, file: usize, properties: Option<&Node>, key: &str) -> Vec<String> {
        let Some(node) = properties.and_then(|p| p.get(key)) else {
            return Vec::new();
        };
        let items: Vec<Node> = match &node.value {
            NodeValue::Array(items) => items.clone(),
            _ => vec![node.clone()],
        };
        let mut out = Vec::new();
        for item in items {
            let Some(text) = item.as_str() else {
                self.error(codes::SYNTAX, format!("`{key}` lists shape ids"), item.span);
                continue;
            };
            let name = Name {
                text: text.to_owned(),
                span: item.span,
            };
            out.push(self.target(file, &name));
        }
        out
    }

    /// A resource's `identifiers` or `properties`: names and targets.
    fn named_ids(
        &mut self,
        file: usize,
        properties: Option<&Node>,
        key: &str,
    ) -> Vec<(String, String)> {
        let Some(NodeValue::Object(members)) =
            properties.and_then(|p| p.get(key)).map(|n| &n.value)
        else {
            return Vec::new();
        };
        let members = members.clone();
        let mut out = Vec::new();
        for (name, value) in members {
            let Some(text) = value.as_str() else {
                continue;
            };
            let target = self.target(
                file,
                &Name {
                    text: text.to_owned(),
                    span: value.span,
                },
            );
            out.push((name.text, target));
        }
        out
    }

    fn draft(&mut self, index: usize) -> Draft {
        let (file, shape, id) = self.declared[index].clone();
        let mut members = Vec::with_capacity(shape.members.len());
        let mut elided = Vec::with_capacity(shape.members.len());
        let mut names = HashSet::new();
        for member in &shape.members {
            if !names.insert(member.name.text.clone()) {
                self.error(
                    codes::DUPLICATE,
                    format!(
                        "member {} is defined twice in {}",
                        member.name.text, shape.name.text
                    ),
                    member.name.span,
                );
                continue;
            }
            let target = match (&member.target, shape.kind) {
                (_, ShapeKind::Enum | ShapeKind::IntEnum) => prelude("Unit"),
                (Some(target), _) => self.target(file, target),
                (None, _) => String::new(),
            };
            elided.push(
                member.target.is_none()
                    && !matches!(shape.kind, ShapeKind::Enum | ShapeKind::IntEnum),
            );
            members.push(RMember {
                name: member.name.text.clone(),
                target,
                traits: self.traits(file, &member.traits),
                default: member.default.clone(),
                doc: member.doc.clone(),
                span: member.span,
            });
        }
        let operation = shape.operation.as_ref().map(|body| {
            let io = |resolver: &mut Self, io: &Option<IoShape>| match io {
                Some(IoShape::Target(name)) => Some(resolver.target(file, name)),
                Some(IoShape::Inline(inline)) => resolver.lookup(file, &inline.name.text),
                None => None,
            };
            ROperation {
                input: io(self, &body.input),
                output: io(self, &body.output),
                errors: body.errors.iter().map(|e| self.target(file, e)).collect(),
            }
        });
        let properties = shape.properties.as_ref();
        let mut bindings = Bindings {
            operations: self.ids(file, properties, "operations"),
            resources: self.ids(file, properties, "resources"),
            errors: self.ids(file, properties, "errors"),
            identifiers: self.named_ids(file, properties, "identifiers"),
            properties: self.named_ids(file, properties, "properties"),
        };
        for key in [
            "collectionOperations",
            "create",
            "put",
            "read",
            "update",
            "delete",
            "list",
        ] {
            let ids = self.ids(file, properties, key);
            bindings.operations.extend(ids);
        }
        let version = properties
            .and_then(|p| p.get("version"))
            .and_then(Node::as_str)
            .map(str::to_owned);
        let mut mixins = Vec::new();
        for mixin in &shape.mixins {
            match self.lookup(file, &mixin.text) {
                Some(id) => mixins.push((id, mixin.span)),
                None => self.error(
                    codes::UNRESOLVED,
                    format!("mixin {} is not defined", mixin.text),
                    mixin.span,
                ),
            }
        }
        let for_resource = shape.for_resource.as_ref().map(|r| self.target(file, r));
        Draft {
            shape: RShape {
                id,
                name: shape.name.text.clone(),
                kind: shape.kind,
                traits: self.traits(file, &shape.traits),
                doc: shape.doc.clone(),
                members,
                operation,
                bindings,
                version,
                span: shape.name.span,
            },
            elided,
            mixins,
            for_resource,
        }
    }

    /// Applies mixins and fills in elided targets.
    fn expand(&mut self, drafts: &[Draft]) -> Vec<RShape> {
        let index: HashMap<String, usize> = drafts
            .iter()
            .enumerate()
            .map(|(i, d)| (d.shape.id.clone(), i))
            .collect();
        let mut done: Vec<Option<RShape>> = vec![None; drafts.len()];
        for i in 0..drafts.len() {
            let mut visiting = HashSet::new();
            self.expand_one(i, drafts, &index, &mut done, &mut visiting);
        }
        done.into_iter()
            .map(|s| s.expect("every shape is expanded"))
            .collect()
    }

    fn expand_one(
        &mut self,
        i: usize,
        drafts: &[Draft],
        index: &HashMap<String, usize>,
        done: &mut Vec<Option<RShape>>,
        visiting: &mut HashSet<usize>,
    ) {
        if done[i].is_some() {
            return;
        }
        let draft = &drafts[i];
        if !visiting.insert(i) {
            self.error(
                codes::INVALID,
                format!("{} is its own mixin", draft.shape.name),
                draft.shape.span,
            );
            done[i] = Some(draft.shape.clone());
            return;
        }
        let mut members: Vec<RMember> = Vec::new();
        let mut traits: Vec<RTrait> = Vec::new();
        for (mixin, span) in &draft.mixins {
            let Some(&m) = index.get(mixin) else {
                // A prelude shape.
                self.error(codes::WRONG_KIND, format!("{mixin} is not a mixin"), *span);
                continue;
            };
            self.expand_one(m, drafts, index, done, visiting);
            let mixed = done[m].clone().expect("expanded above");
            if !mixed.has_trait("mixin") {
                self.error(
                    codes::WRONG_KIND,
                    format!("{} is not a mixin: it has no @mixin trait", mixed.name),
                    *span,
                );
            }
            for member in mixed.members {
                if !members.iter().any(|x| x.name == member.name) {
                    members.push(member);
                }
            }
            for t in mixed.traits {
                if t.id != prelude("mixin") && !traits.iter().any(|x| x.id == t.id) {
                    traits.push(t);
                }
            }
        }
        let mut shape = draft.shape.clone();
        for (own, elided) in shape.members.iter().zip(&draft.elided) {
            let mut own = own.clone();
            if let Some(inherited) = members.iter_mut().find(|m| m.name == own.name) {
                // Redeclaring a mixin's member adds traits to it.
                for t in own.traits {
                    inherited.traits.retain(|x| x.id != t.id);
                    inherited.traits.push(t);
                }
                if !elided {
                    inherited.target = own.target;
                }
                if own.default.is_some() {
                    inherited.default = own.default;
                }
                continue;
            }
            if *elided {
                if let Some(target) =
                    resource_target(draft.for_resource.as_deref(), &own.name, drafts, index)
                {
                    own.target = target;
                } else {
                    self.error(
                        codes::INVALID,
                        format!("no mixin or resource gives ${} a target", own.name),
                        own.span,
                    );
                    own.target = prelude("Document");
                }
            }
            members.push(own);
        }
        for t in &shape.traits {
            traits.retain(|x| x.id != t.id);
        }
        traits.extend(std::mem::take(&mut shape.traits));
        shape.traits = traits;
        shape.members = members;
        visiting.remove(&i);
        done[i] = Some(shape);
    }

    /// Adds the traits of `apply` statements.
    fn apply(&mut self, drafts: &mut [Draft]) {
        let index: HashMap<String, usize> = drafts
            .iter()
            .enumerate()
            .map(|(i, d)| (d.shape.id.clone(), i))
            .collect();
        for (file, smithy) in self.files.iter().enumerate() {
            for apply in &smithy.model.applies {
                let (shape_text, member) = match apply.target.text.split_once('$') {
                    Some((shape, member)) => (shape, Some(member)),
                    None => (apply.target.text.as_str(), None),
                };
                let Some(&i) = self
                    .lookup(file, shape_text)
                    .as_ref()
                    .and_then(|id| index.get(id))
                else {
                    self.error(
                        codes::UNRESOLVED,
                        format!("shape {} is not defined", apply.target.text),
                        apply.target.span,
                    );
                    continue;
                };
                let traits = self.traits(file, &apply.traits);
                let shape = &mut drafts[i].shape;
                let slot = match member {
                    None => &mut shape.traits,
                    Some(member) => {
                        if let Some(m) = shape.members.iter_mut().find(|m| m.name == member) {
                            &mut m.traits
                        } else {
                            let message = format!("{} has no member {member}", shape.name);
                            self.error(codes::UNRESOLVED, message, apply.target.span);
                            continue;
                        }
                    }
                };
                for t in traits {
                    slot.retain(|x| x.id != t.id);
                    slot.push(t);
                }
            }
        }
    }
}

/// The target a resource gives an identifier or property `name`.
fn resource_target(
    resource: Option<&str>,
    name: &str,
    drafts: &[Draft],
    index: &HashMap<String, usize>,
) -> Option<String> {
    let resource = &drafts[*index.get(resource?)?].shape;
    resource
        .bindings
        .identifiers
        .iter()
        .chain(&resource.bindings.properties)
        .find(|(n, _)| n == name)
        .map(|(_, target)| target.clone())
}

/// Operations' inputs and outputs are structures, errors structures with
/// `@error`, and services and resources bind operations and resources.
fn check_kinds(model: &mut Resolved) {
    let mut problems = Vec::new();
    let kind_of = |id: &str| model.get(id).map(|s| (s.kind, s.has_trait("error")));
    for shape in &model.shapes {
        if let Some(op) = &shape.operation {
            for io in [&op.input, &op.output].into_iter().flatten() {
                if let Some((kind, _)) = kind_of(io)
                    && kind != ShapeKind::Structure
                {
                    problems.push((format!("{io} is not a structure"), shape.span));
                }
            }
            for error in &op.errors {
                if let Some((kind, is_error)) = kind_of(error)
                    && (kind != ShapeKind::Structure || !is_error)
                {
                    problems.push((
                        format!("{error} is not a structure with @error"),
                        shape.span,
                    ));
                }
            }
        }
        for op in &shape.bindings.operations {
            if kind_of(op).is_some_and(|(kind, _)| kind != ShapeKind::Operation) {
                problems.push((format!("{op} is not an operation"), shape.span));
            }
        }
        for resource in &shape.bindings.resources {
            if kind_of(resource).is_some_and(|(kind, _)| kind != ShapeKind::Resource) {
                problems.push((format!("{resource} is not a resource"), shape.span));
            }
        }
    }
    for (message, span) in problems {
        model.diagnostics.push(
            Diagnostic::error(message)
                .with_code(codes::WRONG_KIND)
                .with_primary(span, ""),
        );
    }
}
