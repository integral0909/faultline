//! Lowering of applicator keywords and assembly of a schema's IR node.
//!
//! A schema object is the conjunction of everything it says: its references,
//! its typed part, `enum`/`const`, `allOf`, `anyOf`, `oneOf`, `not`,
//! conditionals and dependent schemas. Each contributes one *part*; a single
//! part becomes the node itself, several become an `AllOf`.

use std::collections::HashSet;

use faultline_doc::{Node, UriRef, Value};
use faultline_ir::{
    Discriminator, Field, Meta, ObjectType, TypeId, TypeKind, TypeNode, UnionMode, UnionType,
};
use faultline_source::{Diagnostic, Span};

use crate::assertions::TypeSet;
use crate::codes;
use crate::lower::{Lowerer, SchemaCtx, outermost_with};

/// One conjunct of a schema.
enum Part {
    Kind(Box<TypeKind>),
    Id(TypeId),
}

impl Part {
    fn kind(kind: TypeKind) -> Part {
        Part::Kind(Box::new(kind))
    }
}

/// Keywords that do not assert anything, for the draft-07 `$ref` sibling
/// warning.
const ANNOTATIONS: [&str; 14] = [
    "$ref",
    "$id",
    "id",
    "$schema",
    "$comment",
    "title",
    "description",
    "default",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
    "definitions",
    "$defs",
];

impl Lowerer<'_, '_> {
    pub(crate) fn build_schema(&mut self, ctx: &SchemaCtx<'_>) -> TypeNode {
        let meta = self.meta(ctx);
        let span = ctx.node.span;
        let mut parts = Vec::new();

        if let Some(reference) = ctx.get("$ref") {
            let target = self.static_ref(ctx, reference);
            if ctx.dialect().ref_overrides_siblings() {
                self.warn_ignored_siblings(ctx, reference.span);
                let kind = target.map_or(TypeKind::Any, TypeKind::Ref);
                return finish_node(kind, meta, span);
            }
            parts.extend(target.map(Part::Id));
        }
        if ctx.dialect().has_dynamic_ref()
            && let Some(reference) = ctx.get("$dynamicRef")
        {
            parts.extend(self.dynamic_ref(ctx, reference).map(Part::Id));
        }
        if ctx.dialect().has_recursive_ref()
            && let Some(reference) = ctx.get("$recursiveRef")
        {
            parts.extend(self.recursive_ref(ctx, reference).map(Part::Id));
        }

        self.value_parts(ctx, &mut parts);
        self.combinator_parts(ctx, &mut parts);
        self.conditional_parts(ctx, &mut parts);
        if ctx.dialect().has_2019_keywords() {
            self.unevaluated_parts(ctx, &mut parts);
        }

        let kind = match parts.len() {
            0 => TypeKind::Any,
            1 => match parts.pop().expect("one part") {
                Part::Kind(kind) => *kind,
                Part::Id(id) => TypeKind::Ref(id),
            },
            _ => {
                let ids = parts
                    .into_iter()
                    .map(|part| match part {
                        Part::Kind(kind) => self.alloc(*kind),
                        Part::Id(id) => id,
                    })
                    .collect();
                TypeKind::AllOf(ids)
            }
        };
        finish_node(kind, meta, span)
    }

    /// The typed part and `enum`/`const`. When the typed part only restates
    /// the type of every enum member, it adds nothing and is dropped, so that
    /// `{"type": "string", "enum": ["a"]}` lowers to a plain enum.
    fn value_parts(&mut self, ctx: &SchemaCtx<'_>, parts: &mut Vec<Part>) {
        let typed = self.typed_part(ctx);
        let enums = self.enum_parts(ctx);
        if let Some(typed) = typed {
            let redundant = !typed.constrained
                && !enums.is_empty()
                && enums
                    .iter()
                    .all(|e| e.values.iter().all(|v| typed.types.admits(&v.value)));
            if !redundant {
                parts.push(Part::kind(typed.kind));
            }
        }
        parts.extend(enums.into_iter().map(|e| Part::kind(TypeKind::Enum(e))));
    }

    fn combinator_parts(&mut self, ctx: &SchemaCtx<'_>, parts: &mut Vec<Part>) {
        if let Some(members) = self.schema_list(ctx, "allOf") {
            parts.extend(members.into_iter().map(Part::Id));
        }
        for (keyword, mode) in [("anyOf", UnionMode::Any), ("oneOf", UnionMode::Exclusive)] {
            if let Some(variants) = self.schema_list(ctx, keyword) {
                let discriminator = self.discriminator(ctx, keyword, &variants);
                parts.push(Part::kind(TypeKind::Union(UnionType {
                    mode,
                    variants,
                    discriminator,
                })));
            }
        }
        if ctx.get("not").is_some() {
            let operand = self.child(ctx, &["not"]);
            parts.push(Part::kind(TypeKind::Not(operand)));
        }
    }

    /// `if`/`then`/`else` and dependent schemas, expressed with unions:
    /// `if A then B else C` is `(A and B) or (not A and C)`, and a dependent
    /// schema `{p: S}` is `(p absent) or S`.
    fn conditional_parts(&mut self, ctx: &SchemaCtx<'_>, parts: &mut Vec<Part>) {
        if ctx.dialect().has_conditionals()
            && ctx.get("if").is_some()
            && (ctx.get("then").is_some() || ctx.get("else").is_some())
        {
            let condition = self.child(ctx, &["if"]);
            let then = self.optional_child(ctx, "then");
            let otherwise = self.optional_child(ctx, "else");
            let when_true = self.alloc(TypeKind::AllOf(vec![condition, then]));
            let negated = self.alloc(TypeKind::Not(condition));
            let when_false = self.alloc(TypeKind::AllOf(vec![negated, otherwise]));
            parts.push(Part::kind(TypeKind::Union(UnionType {
                mode: UnionMode::Any,
                variants: vec![when_true, when_false],
                discriminator: None,
            })));
        }

        let keyword = if ctx.dialect().has_2019_keywords() {
            "dependentSchemas"
        } else {
            "dependencies"
        };
        let Some(dependents) = ctx.get(keyword).and_then(Node::as_object) else {
            return;
        };
        let schema_valued: Vec<String> = dependents
            .iter()
            .filter(|(_, v)| !matches!(v.value, Value::Array(_)))
            .map(|(k, _)| k.to_owned())
            .collect();
        for property in schema_valued {
            let schema = self.child(ctx, &[keyword, &property]);
            let present = self.object_with_member(&property);
            let absent = self.alloc(TypeKind::Not(present));
            parts.push(Part::kind(TypeKind::Union(UnionType {
                mode: UnionMode::Any,
                variants: vec![absent, schema],
                discriminator: None,
            })));
        }
    }

    /// An OpenAPI discriminator next to `keyword`'s variants. Explicit
    /// `mapping` entries name schemas by reference or by component name;
    /// variants that reference `#/components/schemas/Name` without an
    /// explicit entry are implicitly mapped from `Name`.
    fn discriminator(
        &mut self,
        ctx: &SchemaCtx<'_>,
        keyword: &str,
        variants: &[TypeId],
    ) -> Option<Discriminator> {
        let node = ctx.get("discriminator")?;
        let Some(property) = node.get("propertyName").and_then(Node::as_str) else {
            self.invalid(
                ctx,
                "discriminator",
                "must be an object with a string propertyName",
            );
            return None;
        };
        let mut mapping = Vec::new();
        let mut explicit_targets = Vec::new();
        if let Some(explicit) = node.get("mapping").and_then(Node::as_object) {
            for (value, target) in explicit.iter() {
                let Some(target_text) = target.as_str() else {
                    continue;
                };
                let reference = if target_text.contains(['/', '#']) {
                    target_text.to_owned()
                } else {
                    format!("#/components/schemas/{target_text}")
                };
                if let Some(location) = self.reference(ctx, &reference, target.span) {
                    let id = self.target(&location, &ctx.scope);
                    mapping.push((value.to_owned(), id));
                    explicit_targets.push(location);
                }
            }
        }
        // A schema that is already the target of an explicit mapping is not
        // also reachable under its implicit name.
        for (i, &variant) in variants.iter().enumerate() {
            let reference_node = ctx.node_at(&[keyword, &i.to_string(), "$ref"]);
            let Some(reference) = reference_node.and_then(Node::as_str) else {
                continue;
            };
            let Some(name) = reference.strip_prefix("#/components/schemas/") else {
                continue;
            };
            if name.contains('/') || mapping.iter().any(|(value, _)| value == name) {
                continue;
            }
            let span = reference_node.map_or(node.span, |n| n.span);
            let explicitly_mapped = self
                .reference(ctx, reference, span)
                .is_some_and(|location| explicit_targets.contains(&location));
            if !explicitly_mapped {
                mapping.push((name.to_owned(), variant));
            }
        }
        Some(Discriminator {
            property: property.to_owned(),
            mapping,
        })
    }

    /// Objects that have a member named `name`, of any value.
    fn object_with_member(&mut self, name: &str) -> TypeId {
        let any = self.any();
        let mut object = ObjectType::new(any);
        object.fields.push(Field::new(name, any, true));
        self.alloc(TypeKind::Object(object))
    }

    fn schema_list(&mut self, ctx: &SchemaCtx<'_>, keyword: &str) -> Option<Vec<TypeId>> {
        let node = ctx.get(keyword)?;
        match node.as_array() {
            Some(items) if !items.is_empty() => Some(
                (0..items.len())
                    .map(|i| self.child(ctx, &[keyword, &i.to_string()]))
                    .collect(),
            ),
            _ => {
                self.invalid(ctx, keyword, "must be a non-empty array of schemas");
                None
            }
        }
    }

    fn static_ref(&mut self, ctx: &SchemaCtx<'_>, node: &Node) -> Option<TypeId> {
        let Some(text) = node.as_str() else {
            self.invalid(ctx, "$ref", "must be a string");
            return None;
        };
        let location = self.reference(ctx, text, node.span)?;
        Some(self.target(&location, &ctx.scope))
    }

    /// `$dynamicRef` (2020-12): resolves like `$ref`, but if the initial
    /// target declares a matching `$dynamicAnchor`, the outermost resource in
    /// the dynamic scope that declares the same anchor wins.
    fn dynamic_ref(&mut self, ctx: &SchemaCtx<'_>, node: &Node) -> Option<TypeId> {
        let Some(text) = node.as_str() else {
            self.invalid(ctx, "$dynamicRef", "must be a string");
            return None;
        };
        let initial = self.reference(ctx, text, node.span)?;
        let anchor = UriRef::parse(text)
            .ok()
            .and_then(|u| u.fragment().map(str::to_owned))
            .filter(|f| !f.is_empty() && !f.starts_with('/'));
        let target = anchor
            .and_then(|name| {
                let info = self.registry.info(&initial)?;
                let resource = self.registry.resource(&info.resource)?;
                if resource.dynamic_anchors.get(&name) != Some(&initial) {
                    return None;
                }
                let outermost = outermost_with(&self.registry, &ctx.scope, |r| {
                    r.dynamic_anchors.contains_key(&name)
                })?;
                self.registry
                    .resource(&outermost)?
                    .dynamic_anchors
                    .get(&name)
                    .cloned()
            })
            .unwrap_or(initial);
        Some(self.target(&target, &ctx.scope))
    }

    /// `$recursiveRef` (2019-09): only `"#"` is allowed. If the current
    /// resource has `$recursiveAnchor: true`, the reference goes to the
    /// outermost such resource in the dynamic scope.
    fn recursive_ref(&mut self, ctx: &SchemaCtx<'_>, node: &Node) -> Option<TypeId> {
        if node.as_str() != Some("#") {
            self.invalid(ctx, "$recursiveRef", "must be \"#\"");
            return None;
        }
        let initial = self.reference(ctx, "#", node.span)?;
        let current = self.registry.resource(&ctx.info.resource);
        let target = if current.is_some_and(|r| r.recursive_anchor) {
            outermost_with(&self.registry, &ctx.scope, |r| r.recursive_anchor)
                .and_then(|uri| self.registry.resource(&uri).map(|r| r.root.clone()))
                .unwrap_or(initial)
        } else {
            initial
        };
        Some(self.target(&target, &ctx.scope))
    }

    fn warn_ignored_siblings(&mut self, ctx: &SchemaCtx<'_>, ref_span: Span) {
        let Some(obj) = ctx.node.as_object() else {
            return;
        };
        let ignored: Vec<&str> = obj
            .keys()
            .filter(|k| !ANNOTATIONS.contains(k) && !k.starts_with("x-"))
            .collect();
        if !ignored.is_empty() {
            self.diagnostics.push(
                Diagnostic::warning(format!(
                    "keywords next to $ref are ignored in {}",
                    ctx.dialect()
                ))
                .with_code(codes::REF_SIBLINGS_IGNORED)
                .with_primary(ref_span, format!("ignored: {}", ignored.join(", ")))
                .with_help("wrap the reference in allOf to combine it with other keywords"),
            );
        }
    }

    /// `unevaluatedProperties`/`unevaluatedItems` depend on which members the
    /// rest of the schema evaluated, which in general depends on the instance.
    /// They are approximated statically: members named by any in-place
    /// subschema count as evaluated. This is exact when those subschemas are
    /// reached through `allOf` and `$ref` only; with branches (`anyOf`,
    /// `oneOf`, conditionals) the IR accepts more values than the schema, and
    /// a warning says so.
    fn unevaluated_parts(&mut self, ctx: &SchemaCtx<'_>, parts: &mut Vec<Part>) {
        if ctx.get("unevaluatedProperties").is_some() {
            let evaluated = self.collect_evaluated(ctx);
            if !evaluated.all_properties {
                let additional = self.child(ctx, &["unevaluatedProperties"]);
                let any = self.any();
                let mut object = ObjectType::new(additional);
                object.fields = evaluated
                    .names
                    .iter()
                    .map(|name| Field::new(name.clone(), any, false))
                    .collect();
                object.pattern_properties = evaluated
                    .patterns
                    .iter()
                    .map(|p| (p.clone(), any))
                    .collect();
                parts.push(Part::kind(
                    self.only_for(TypeSet::OBJECT, TypeKind::Object(object)),
                ));
                self.warn_if_approximate(ctx, "unevaluatedProperties", evaluated.branched);
            }
        }
        if ctx.get("unevaluatedItems").is_some() {
            let evaluated = self.collect_evaluated(ctx);
            if !evaluated.all_items {
                let items = self.child(ctx, &["unevaluatedItems"]);
                let any = self.any();
                let mut array = faultline_ir::ArrayType::of(items);
                array.prefix_items = vec![any; evaluated.prefix_items];
                parts.push(Part::kind(
                    self.only_for(TypeSet::ARRAY, TypeKind::Array(array)),
                ));
                self.warn_if_approximate(ctx, "unevaluatedItems", evaluated.branched);
            }
        }
    }

    /// `kind` restricted to values of type `only`: other values pass.
    fn only_for(&mut self, only: TypeSet, kind: TypeKind) -> TypeKind {
        let any = self.any();
        let unconstrained = if only == TypeSet::OBJECT {
            TypeKind::Object(ObjectType::new(any))
        } else {
            TypeKind::Array(faultline_ir::ArrayType::of(any))
        };
        let all_of_type = self.alloc(unconstrained);
        let others = self.alloc(TypeKind::Not(all_of_type));
        let constrained = self.alloc(kind);
        TypeKind::Union(UnionType {
            mode: UnionMode::Any,
            variants: vec![others, constrained],
            discriminator: None,
        })
    }

    fn warn_if_approximate(&mut self, ctx: &SchemaCtx<'_>, keyword: &str, branched: bool) {
        if branched && let Some(node) = ctx.get(keyword) {
            self.diagnostics.push(
                Diagnostic::warning(format!("{keyword} is approximated"))
                    .with_code(codes::APPROXIMATED)
                    .with_primary(node.span, "members evaluated only by some branches are treated as always evaluated")
                    .with_note("compatibility results for this schema may miss changes that only matter for some instances"),
            );
        }
    }

    /// Walks the in-place subschemas of `ctx` (following `$ref`) and collects
    /// what they evaluate.
    fn collect_evaluated(&mut self, ctx: &SchemaCtx<'_>) -> Evaluated {
        const MAX_SCHEMAS: usize = 256;
        let mut evaluated = Evaluated::default();
        let mut visited = HashSet::new();
        let mut stack = vec![(ctx.location.clone(), false)];
        let dialect = ctx.dialect();
        while let Some((location, branch)) = stack.pop() {
            if !visited.insert(location.clone()) || visited.len() > MAX_SCHEMAS {
                continue;
            }
            let Some(document) = self.registry.document(&location.document) else {
                continue;
            };
            let Some(obj) = document
                .document
                .root
                .pointer(&location.pointer)
                .and_then(Node::as_object)
            else {
                continue;
            };
            let is_root = location == ctx.location;
            if evaluated.record(obj, is_root, dialect) && branch {
                evaluated.branched = true;
            }

            let mut push = |tokens: &[&str], branch: bool| {
                stack.push((location.join(tokens.iter().copied()), branch));
            };
            if let Some(list) = obj.get("allOf").and_then(Node::as_array) {
                for i in 0..list.len() {
                    push(&["allOf", &i.to_string()], branch);
                }
            }
            for keyword in ["anyOf", "oneOf"] {
                if let Some(list) = obj.get(keyword).and_then(Node::as_array) {
                    for i in 0..list.len() {
                        push(&[keyword, &i.to_string()], true);
                    }
                }
            }
            for keyword in ["if", "then", "else"] {
                if obj.contains_key(keyword) {
                    push(&[keyword], true);
                }
            }
            if let Some(dependents) = obj.get("dependentSchemas").and_then(Node::as_object) {
                for key in dependents.keys() {
                    push(&["dependentSchemas", key], true);
                }
            }
            if let Some(reference) = obj.get("$ref").and_then(Node::as_str)
                && let Some(info) = self.registry.info(&location).cloned()
                && let Ok(target) =
                    self.registry
                        .resolve(self.store, reference, &info, &mut self.diagnostics)
            {
                stack.push((target, branch));
            }
        }
        evaluated.names.sort();
        evaluated.names.dedup();
        evaluated.patterns.sort();
        evaluated.patterns.dedup();
        evaluated
    }
}

#[derive(Default)]
struct Evaluated {
    names: Vec<String>,
    patterns: Vec<String>,
    prefix_items: usize,
    all_properties: bool,
    all_items: bool,
    /// Some contribution came from a branch that does not always apply.
    branched: bool,
}

impl Evaluated {
    /// Records what the schema object `obj` evaluates by itself. Returns
    /// whether it contributed any member names or tuple positions.
    fn record(
        &mut self,
        obj: &faultline_doc::Object,
        is_root: bool,
        dialect: crate::Dialect,
    ) -> bool {
        let before = (self.names.len(), self.patterns.len(), self.prefix_items);
        if let Some(properties) = obj.get("properties").and_then(Node::as_object) {
            self.names.extend(properties.keys().map(str::to_owned));
        }
        if let Some(patterns) = obj.get("patternProperties").and_then(Node::as_object) {
            self.patterns.extend(patterns.keys().map(str::to_owned));
        }
        // In a subschema, additional/unevaluated properties evaluate every
        // remaining member. At the root they are what is being computed.
        if !is_root
            && (obj.contains_key("additionalProperties")
                || obj.contains_key("unevaluatedProperties"))
        {
            self.all_properties = true;
        }
        let tuple_keyword = if dialect.has_prefix_items() {
            "prefixItems"
        } else {
            "items"
        };
        let prefix = obj
            .get(tuple_keyword)
            .and_then(Node::as_array)
            .map_or(0, <[Node]>::len);
        self.prefix_items = self.prefix_items.max(prefix);
        let items_schema = if dialect.has_prefix_items() {
            obj.contains_key("items")
        } else {
            obj.get("items").is_some_and(|n| n.as_array().is_none())
                || obj.contains_key("additionalItems")
        };
        if items_schema || (!is_root && obj.contains_key("unevaluatedItems")) {
            self.all_items = true;
        }
        before != (self.names.len(), self.patterns.len(), self.prefix_items)
    }
}

fn finish_node(kind: TypeKind, meta: Meta, span: Span) -> TypeNode {
    let mut node = TypeNode::new(kind).with_origin(span);
    node.meta = meta;
    node
}
