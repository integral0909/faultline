//! Matching union variants between versions.
//!
//! Variants have no names in most formats, so they are matched by what they
//! are: first by named type (a `$ref` to the same definition), then by JSON
//! type when exactly one variant on each side has that type, and finally in
//! order among variants of the same kind. Whatever remains unmatched was
//! added or removed.

use faultline_ir::{Api, TypeId, TypeKind};

/// Pairs of (old index, new index); one side is `None` for removed or added
/// variants. Matched pairs come first, in old order.
pub(crate) fn align_variants(
    old: &Api,
    a: &[TypeId],
    new: &Api,
    b: &[TypeId],
) -> Vec<(Option<usize>, Option<usize>)> {
    let mut matched_old = vec![None; a.len()];
    let mut taken_new = vec![false; b.len()];

    let names_a: Vec<Option<String>> = a.iter().map(|&t| name_of(old, t)).collect();
    let names_b: Vec<Option<String>> = b.iter().map(|&t| name_of(new, t)).collect();
    for (i, name) in names_a.iter().enumerate() {
        let Some(name) = name else { continue };
        if let Some(j) = (0..b.len()).find(|&j| !taken_new[j] && names_b[j].as_ref() == Some(name))
        {
            matched_old[i] = Some(j);
            taken_new[j] = true;
        }
    }

    let class_a: Vec<Option<&str>> = a.iter().map(|&t| class_of(old, t)).collect();
    let class_b: Vec<Option<&str>> = b.iter().map(|&t| class_of(new, t)).collect();
    // A JSON type shared by exactly one unmatched variant on each side.
    for i in 0..a.len() {
        if matched_old[i].is_some() {
            continue;
        }
        let Some(class) = class_a[i] else { continue };
        let olds = (0..a.len())
            .filter(|&k| matched_old[k].is_none() && class_a[k] == Some(class))
            .count();
        let news: Vec<usize> = (0..b.len())
            .filter(|&k| !taken_new[k] && class_b[k] == Some(class))
            .collect();
        if olds == 1 && news.len() == 1 {
            matched_old[i] = Some(news[0]);
            taken_new[news[0]] = true;
        }
    }

    // Remaining variants of the same kind, in order.
    for i in 0..a.len() {
        if matched_old[i].is_some() || names_a[i].is_some() {
            continue;
        }
        let kind = kind_of(old, a[i]);
        if let Some(j) = (0..b.len())
            .find(|&j| !taken_new[j] && names_b[j].is_none() && kind_of(new, b[j]) == kind)
        {
            matched_old[i] = Some(j);
            taken_new[j] = true;
        }
    }

    let mut pairs: Vec<(Option<usize>, Option<usize>)> = matched_old
        .iter()
        .enumerate()
        .map(|(i, j)| (Some(i), *j))
        .collect();
    pairs.extend(
        (0..b.len())
            .filter(|&j| !taken_new[j])
            .map(|j| (None, Some(j))),
    );
    pairs
}

fn name_of(api: &Api, ty: TypeId) -> Option<String> {
    // Follow references until a named node or a non-reference.
    let mut id = ty;
    for _ in 0..api.types.len() {
        let node = api.types.get(id);
        if let Some(name) = &node.name {
            return Some(name.to_string());
        }
        match node.kind {
            TypeKind::Ref(target) => id = target,
            _ => return None,
        }
    }
    None
}

fn kind_of(api: &Api, ty: TypeId) -> &'static str {
    api.types.get(api.types.resolve(ty)).kind.describe()
}

/// The JSON type of a variant's values, when it has exactly one.
fn class_of(api: &Api, ty: TypeId) -> Option<&'static str> {
    Some(match &api.types.get(api.types.resolve(ty)).kind {
        TypeKind::Null => "null",
        TypeKind::Boolean => "boolean",
        TypeKind::Integer(_) | TypeKind::Number(_) => "number",
        TypeKind::String(_) | TypeKind::Bytes => "string",
        TypeKind::Array(_) => "array",
        TypeKind::Object(_) | TypeKind::Map(_) => "object",
        _ => return None,
    })
}
