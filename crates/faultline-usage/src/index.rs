//! The usage index: which operations, parameters and fields clients use.
//!
//! An index records counts only: operation identities (templated, never
//! concrete URLs), parameter names, field paths within responses and
//! arguments. Query values, headers, bodies and client identities are
//! never stored.

use std::collections::BTreeMap;

use faultline_ir::{IrPath, Segment};
use serde::{Deserialize, Serialize};

/// Version of the index file format.
pub const INDEX_VERSION: u32 = 1;

/// Usage of one operation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationUsage {
    pub calls: u64,
    /// Parameter uses by `location:name`, as in `param[query:limit]`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, u64>,
    /// Uses of response fields and arguments by dotted path from the
    /// response root (`owner.name`, `nicknames().limit`). Only sources
    /// that see selections (GraphQL documents) record fields.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, u64>,
}

/// Where usage data came from, which decides what absence means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// Server access logs: calls and parameter names, no fields.
    AccessLog,
    /// Client documents: the fields and arguments every operation selects.
    GraphqlDocuments,
    /// Recorded HTTP traffic: calls, parameter names, and the members of
    /// request bodies. What clients read from responses is not known.
    Har,
}

/// Usage of one API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageIndex {
    pub version: u32,
    pub source: Source,
    /// Free-form description of the period or corpus covered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Every operation of the API, including those never called.
    pub operations: BTreeMap<String, OperationUsage>,
    /// Log lines or documents that matched no operation.
    #[serde(default)]
    pub unmatched: u64,
}

/// What the index says about a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observed {
    /// Used this many times.
    Used(u64),
    /// Covered by the index and never used.
    Unused,
    /// The index says nothing about it.
    Unknown,
}

impl UsageIndex {
    pub fn new(source: Source) -> Self {
        Self {
            version: INDEX_VERSION,
            source,
            window: None,
            operations: BTreeMap::new(),
            unmatched: 0,
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let index: UsageIndex =
            serde_json::from_str(text).map_err(|e| format!("invalid usage index: {e}"))?;
        if index.version != INDEX_VERSION {
            return Err(format!(
                "usage index version {} is not supported (expected {INDEX_VERSION})",
                index.version
            ));
        }
        Ok(index)
    }

    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("indexes always serialize");
        text.push('\n');
        text
    }

    /// Merges `other`'s counts into this index.
    pub fn merge(&mut self, other: &UsageIndex) {
        for (key, usage) in &other.operations {
            let entry = self.operations.entry(key.clone()).or_default();
            entry.calls += usage.calls;
            for (k, n) in &usage.parameters {
                *entry.parameters.entry(k.clone()).or_default() += n;
            }
            for (k, n) in &usage.fields {
                *entry.fields.entry(k.clone()).or_default() += n;
            }
        }
        self.unmatched += other.unmatched;
    }

    /// Usage of the position a finding's path names: the operation, one of
    /// its parameters, or (for sources that record selections) a response
    /// field or argument.
    pub fn observe(&self, path: &IrPath) -> Observed {
        let segments = path.segments();
        let Some(Segment::Operation(identity)) = segments.first() else {
            return Observed::Unknown;
        };
        let Some(usage) = self.operations.get(identity) else {
            return Observed::Unknown;
        };
        if usage.calls == 0 {
            return Observed::Unused;
        }
        if let Some(Segment::Parameter(location, name)) = segments.get(1) {
            return count(usage.parameters.get(&format!("{location}:{name}")).copied());
        }
        let chain = field_chain(segments);
        // Field paths are known everywhere for GraphQL documents, and for
        // request bodies (`()...`) in recorded traffic.
        let fields_known = match self.source {
            Source::GraphqlDocuments => true,
            Source::Har => chain.starts_with("()"),
            Source::AccessLog => false,
        };
        if !fields_known || chain.is_empty() {
            return Observed::Used(usage.calls);
        }
        // A field is used if it, or anything under it, is selected.
        let used: u64 = usage
            .fields
            .iter()
            .filter(|(key, _)| {
                key.as_str() == chain
                    || key.starts_with(&chain)
                        && matches!(key.as_bytes().get(chain.len()), Some(b'.' | b'('))
            })
            .map(|(_, n)| *n)
            .sum();
        count(Some(used))
    }
}

fn count(n: Option<u64>) -> Observed {
    match n {
        Some(0) | None => Observed::Unused,
        Some(n) => Observed::Used(n),
    }
}

/// The dotted chain of fields and arguments below an operation's response
/// or request: `owner.name`, `nicknames().limit`. Structural segments
/// (union variants, list items, nullability) are skipped; for request
/// bodies (root arguments) the chain starts with `()`.
pub fn field_chain(segments: &[Segment]) -> String {
    let mut chain = String::new();
    // Root arguments (the request body) are `()name`; a field's arguments
    // are `field().name`.
    let mut in_request = false;
    for segment in &segments[1..] {
        let name = match segment {
            Segment::RequestBody(_) => {
                chain.push_str("()");
                in_request = true;
                continue;
            }
            Segment::Field(name) if name != "__typename" => name,
            Segment::Arguments(name) => {
                if !chain.is_empty() && !in_request {
                    chain.push('.');
                }
                in_request = false;
                chain.push_str(name);
                chain.push_str("()");
                continue;
            }
            _ => continue,
        };
        if !chain.is_empty() && !in_request {
            chain.push('.');
        }
        in_request = false;
        chain.push_str(name);
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> IrPath {
        text.parse().unwrap()
    }

    #[test]
    fn chains_skip_structure() {
        let chain = |p: &str| field_chain(path(p).segments());
        assert_eq!(
            chain("op[query/pet]/response[default]/body[application/graphql]|0.owner|0.name"),
            "owner.name"
        );
        assert_eq!(
            chain("op[query/pet]/response[default]/body[application/graphql]|0.nicknames().limit"),
            "nicknames().limit"
        );
        assert_eq!(
            chain("op[query/pets]/request[application/graphql].first"),
            "()first"
        );
        assert_eq!(chain("op[query/pets]"), "");
    }

    #[test]
    fn observation() {
        let mut index = UsageIndex::new(Source::GraphqlDocuments);
        index
            .operations
            .insert("query/unused".into(), OperationUsage::default());
        let mut pet = OperationUsage {
            calls: 3,
            ..OperationUsage::default()
        };
        pet.fields.insert("owner.name".into(), 2);
        pet.fields.insert("()id".into(), 3);
        index.operations.insert("query/pet".into(), pet);
        let at = |p: &str| index.observe(&path(p));
        assert_eq!(at("op[query/unused]"), Observed::Unused);
        assert_eq!(at("op[query/other]"), Observed::Unknown);
        assert_eq!(at("op[query/pet]"), Observed::Used(3));
        let body = "op[query/pet]/response[default]/body[application/graphql]|0";
        assert_eq!(
            at(&format!("{body}.owner")),
            Observed::Used(2),
            "a parent of a used field"
        );
        assert_eq!(at(&format!("{body}.owner|0.name")), Observed::Used(2));
        assert_eq!(at(&format!("{body}.name")), Observed::Unused);
        assert_eq!(
            at("op[query/pet]/request[application/graphql].id"),
            Observed::Used(3)
        );
        assert_eq!(
            at("op[query/pet]/request[application/graphql].locale"),
            Observed::Unused
        );
    }

    #[test]
    fn recorded_traffic_knows_request_fields_only() {
        let mut index = UsageIndex::new(Source::Har);
        let mut create = OperationUsage {
            calls: 2,
            ..OperationUsage::default()
        };
        create.fields.insert("()name".into(), 2);
        index.operations.insert("POST /pets".into(), create);
        let at = |p: &str| index.observe(&path(p));
        let request = "op[POST /pets]/request[application/json]";
        assert_eq!(at(&format!("{request}.name")), Observed::Used(2));
        assert_eq!(at(&format!("{request}.tag")), Observed::Unused);
        assert_eq!(
            at("op[POST /pets]/response[201]/body[application/json].id"),
            Observed::Used(2),
            "responses say nothing about what clients read"
        );
    }

    #[test]
    fn round_trip_and_merge() {
        let mut a = UsageIndex::new(Source::AccessLog);
        a.operations.insert(
            "GET /pets".into(),
            OperationUsage {
                calls: 2,
                parameters: BTreeMap::from([("query:limit".into(), 1)]),
                fields: BTreeMap::new(),
            },
        );
        let parsed = UsageIndex::parse(&a.to_json()).unwrap();
        assert_eq!(parsed, a);
        let mut merged = a.clone();
        merged.merge(&a);
        assert_eq!(merged.operations["GET /pets"].calls, 4);
        assert_eq!(merged.operations["GET /pets"].parameters["query:limit"], 2);
        assert!(
            UsageIndex::parse(r#"{"version": 9, "source": "access-log", "operations": {}}"#)
                .is_err()
        );
    }
}
