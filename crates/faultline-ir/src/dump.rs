use std::collections::HashMap;
use std::fmt::Write as _;

use crate::api::Api;
use crate::arena::TypeId;
use crate::graph::edges;
use crate::polarity::Polarities;
use crate::types::{
    Access, ArrayType, Bound, EnumType, Meta, NumericType, ObjectType, StringType, TypeKind,
    UnionMode, UnionType,
};

/// Renders `api` deterministically for humans and snapshot tests.
///
/// The dump backs `faultline ir dump` and is the form IR snapshots are
/// written in. It depends only on the logical structure of the API, never on
/// arena indices, so two lowerings of equivalent input dump identically.
///
/// ```text
/// api petstore (openapi-3.1) version 1.0.0
///
/// op[GET /pets/{}]/param[query:limit] [in] -> t1
/// op[GET /pets/{}]/response[200]/body[application/json] [out] -> t2
///
/// t1 [in]: integer format=int32 min=1 max=100
/// t2 [out]: array items=Pet
/// Pet [out]: object additional=t4
///   .name required: t3
/// t3 [out]: string
/// t4 [out]: any
/// ```
///
/// Named types are labelled by name; other types get `tN` labels in the order
/// a depth-first walk from the roots first reaches them. Named types not
/// reachable from any root are listed at the end.
pub fn dump(api: &Api) -> String {
    let polarities = Polarities::compute(api);
    let roots = api.roots();

    let mut order = Vec::new();
    let mut labels = HashMap::new();
    let mut counter = 0usize;
    let starts = roots
        .iter()
        .map(|r| r.ty)
        .chain(api.named.values().copied());
    for start in starts {
        visit(api, start, &mut labels, &mut order, &mut counter);
    }

    let mut out = String::new();
    let _ = write!(out, "api {} ({})", api.name, api.format);
    if let Some(version) = &api.version {
        let _ = write!(out, " version {version}");
    }
    out.push('\n');

    let streams: Vec<String> = api
        .operations
        .iter()
        .filter_map(|op| {
            let s = op.streaming.filter(|s| s.request || s.response)?;
            let sides = match (s.request, s.response) {
                (true, true) => "request, response",
                (true, false) => "request",
                _ => "response",
            };
            Some(format!("op[{}] streams {sides}", op.key.identity()))
        })
        .collect();
    if !streams.is_empty() {
        out.push('\n');
        for line in streams {
            out.push_str(&line);
            out.push('\n');
        }
    }

    if !roots.is_empty() {
        out.push('\n');
        for root in &roots {
            let _ = writeln!(
                out,
                "{} [{}] -> {}",
                root.path, root.polarity, labels[&root.ty]
            );
        }
    }
    if !order.is_empty() {
        out.push('\n');
        let labels = Labels(labels);
        for id in order {
            write_type(api, id, &labels, &polarities, &mut out);
        }
    }
    out
}

/// Assigns labels in depth-first preorder, iteratively so deep graphs cannot
/// exhaust the stack.
fn visit(
    api: &Api,
    start: TypeId,
    labels: &mut HashMap<TypeId, String>,
    order: &mut Vec<TypeId>,
    counter: &mut usize,
) {
    let mut stack = vec![start];
    while let Some(id) = stack.pop() {
        if labels.contains_key(&id) {
            continue;
        }
        let node = api.types.get(id);
        let label = if let Some(name) = &node.name {
            name.to_string()
        } else {
            *counter += 1;
            format!("t{counter}")
        };
        labels.insert(id, label);
        order.push(id);
        // Reverse so the first child is visited first.
        for (_, child) in edges(node).into_iter().rev() {
            stack.push(child);
        }
    }
}

/// Type labels assigned by [`visit`].
struct Labels(HashMap<TypeId, String>);

impl Labels {
    fn of(&self, id: TypeId) -> &str {
        &self.0[&id]
    }

    fn list(&self, ids: &[TypeId]) -> String {
        ids.iter()
            .map(|&id| self.of(id))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Writes one type as a header line plus indented member lines.
fn write_type(api: &Api, id: TypeId, labels: &Labels, polarities: &Polarities, out: &mut String) {
    let node = api.types.get(id);
    let _ = write!(
        out,
        "{} [{}]: {}",
        labels.of(id),
        polarities.get(id),
        node.kind.describe()
    );
    let mut members = Vec::new();
    match &node.kind {
        TypeKind::Any | TypeKind::Never | TypeKind::Null | TypeKind::Boolean | TypeKind::Bytes => {}
        TypeKind::Integer(n) | TypeKind::Number(n) => write_numeric(n, out),
        TypeKind::String(s) => write_string(s, out),
        TypeKind::Enum(e) => write_enum(e, out, &mut members),
        TypeKind::Object(obj) => write_object(obj, labels, out, &mut members),
        TypeKind::Array(arr) => write_array(arr, labels, out),
        TypeKind::Map(map) => {
            let _ = write!(
                out,
                " key={} value={}",
                labels.of(map.key),
                labels.of(map.value)
            );
        }
        TypeKind::Union(u) => write_union(u, labels, out, &mut members),
        TypeKind::AllOf(parts) => {
            let _ = write!(out, " [{}]", labels.list(parts));
        }
        TypeKind::Not(operand) | TypeKind::Ref(operand) => {
            let _ = write!(out, " {}", labels.of(*operand));
        }
    }
    write_meta(&node.meta, out);
    out.push('\n');
    for member in members {
        let _ = writeln!(out, "  {member}");
    }
}

fn write_string(s: &StringType, out: &mut String) {
    if s.min_length > 0 {
        let _ = write!(out, " minLength={}", s.min_length);
    }
    if let Some(max) = s.max_length {
        let _ = write!(out, " maxLength={max}");
    }
    if let Some(pattern) = &s.pattern {
        let _ = write!(out, " pattern={pattern:?}");
    }
    if let Some(format) = &s.format {
        let _ = write!(out, " format={format}");
    }
}

fn write_enum(e: &EnumType, out: &mut String, members: &mut Vec<String>) {
    if e.open {
        out.push_str(" open");
    }
    for value in &e.values {
        let mut line = format!("= {}", value.value);
        if let Some(name) = &value.name {
            let _ = write!(line, " name={name}");
        }
        if let Some(number) = value.number {
            let _ = write!(line, " number={number}");
        }
        write_meta(&value.meta, &mut line);
        members.push(line);
    }
}

fn write_object(obj: &ObjectType, labels: &Labels, out: &mut String, members: &mut Vec<String>) {
    let _ = write!(out, " additional={}", labels.of(obj.additional));
    if let Some(names) = obj.property_names {
        let _ = write!(out, " propertyNames={}", labels.of(names));
    }
    if obj.min_properties > 0 {
        let _ = write!(out, " minProperties={}", obj.min_properties);
    }
    if let Some(max) = obj.max_properties {
        let _ = write!(out, " maxProperties={max}");
    }
    for field in &obj.fields {
        let mut line = format!(".{}", field.name);
        if field.required {
            line.push_str(" required");
        }
        if let Some(tag) = field.tag {
            let _ = write!(line, " tag={tag}");
        }
        if !field.aliases.is_empty() {
            let _ = write!(line, " aliases={}", field.aliases.join(","));
        }
        write_meta(&field.meta, &mut line);
        let _ = write!(line, ": {}", labels.of(field.ty));
        members.push(line);
    }
    for (pattern, ty) in &obj.pattern_properties {
        members.push(format!("~{pattern:?}: {}", labels.of(*ty)));
    }
    for (property, required) in &obj.dependent_required {
        members.push(format!(
            "if .{property} then required {}",
            required.join(", ")
        ));
    }
    for group in &obj.exclusive {
        members.push(format!(
            "at most one of {} ({})",
            group.fields.join(", "),
            group.name
        ));
    }
}

fn write_array(arr: &ArrayType, labels: &Labels, out: &mut String) {
    let _ = write!(out, " items={}", labels.of(arr.items));
    if !arr.prefix_items.is_empty() {
        let _ = write!(out, " prefix=[{}]", labels.list(&arr.prefix_items));
    }
    if arr.min_items > 0 {
        let _ = write!(out, " minItems={}", arr.min_items);
    }
    if let Some(max) = arr.max_items {
        let _ = write!(out, " maxItems={max}");
    }
    if arr.unique_items {
        out.push_str(" unique");
    }
    if let Some(c) = &arr.contains {
        let _ = write!(out, " contains={} minContains={}", labels.of(c.ty), c.min);
        if let Some(max) = c.max {
            let _ = write!(out, " maxContains={max}");
        }
    }
}

fn write_union(u: &UnionType, labels: &Labels, out: &mut String, members: &mut Vec<String>) {
    let mode = match u.mode {
        UnionMode::Any => "any",
        UnionMode::Exclusive => "exclusive",
        UnionMode::Tagged => "tagged",
    };
    let _ = write!(out, " {mode} [{}]", labels.list(&u.variants));
    if let Some(d) = &u.discriminator {
        let _ = write!(out, " discriminator={}", d.property);
        for (value, ty) in &d.mapping {
            members.push(format!("{value:?} => {}", labels.of(*ty)));
        }
    }
}

fn write_numeric(n: &NumericType, out: &mut String) {
    if let Some(format) = &n.format {
        let _ = write!(out, " format={format}");
    }
    if let Some(encoding) = &n.encoding {
        let _ = write!(out, " encoding={encoding}");
    }
    let bound = |name: &str, b: &Bound| {
        if b.exclusive {
            format!(" exclusive{name}={}", b.value)
        } else {
            format!(" {}={}", name.to_ascii_lowercase(), b.value)
        }
    };
    if let Some(min) = &n.minimum {
        out.push_str(&bound("Min", min));
    }
    if let Some(max) = &n.maximum {
        out.push_str(&bound("Max", max));
    }
    if let Some(m) = &n.multiple_of {
        let _ = write!(out, " multipleOf={m}");
    }
}

fn write_meta(meta: &Meta, out: &mut String) {
    match meta.access {
        Access::ReadWrite => {}
        Access::ReadOnly => out.push_str(" readOnly"),
        Access::WriteOnly => out.push_str(" writeOnly"),
    }
    if meta.deprecated {
        out.push_str(" deprecated");
    }
    if let Some(default) = &meta.default {
        let _ = write!(out, " default={default}");
    }
}
