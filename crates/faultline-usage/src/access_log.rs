//! Importing server access logs.
//!
//! Each request line is matched to the API operation whose method and path
//! template it fits; the most specific template wins (`/pets/mine` over
//! `/pets/{id}`). Only the operation, its call count and the *names* of
//! query parameters are recorded.
//!
//! Two line formats are recognized: the Common and Combined Log Formats
//! (`... "GET /pets?limit=5 HTTP/1.1" 200 ...`) and JSON objects with a
//! `method` and a `path`, `url`, `uri` or `request_uri` (or a `request`
//! holding the request line). Other lines are counted as unmatched.

use faultline_ir::{Api, OperationKey};

use crate::index::{OperationUsage, Source, UsageIndex};

/// Options for matching requests to operations.
#[derive(Clone, Debug, Default)]
pub struct AccessLogOptions {
    /// A prefix the server serves the API under (`/v1`), removed from
    /// request paths before matching.
    pub base_path: Option<String>,
}

/// One HTTP operation's matcher.
struct Route {
    method: String,
    segments: Vec<Vec<Part>>,
    identity: String,
    /// Literal segments, for preferring specific templates.
    specificity: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Literal(String),
    Hole,
}

/// Imports `lines` against the HTTP operations of `api`.
pub fn import<S: AsRef<str>>(
    api: &Api,
    lines: impl IntoIterator<Item = S>,
    options: &AccessLogOptions,
) -> UsageIndex {
    let mut index = UsageIndex::new(Source::AccessLog);
    let mut routes: Vec<Route> = api
        .operations
        .iter()
        .filter_map(|op| match &op.key {
            OperationKey::Http { method, path } => {
                let segments: Vec<Vec<Part>> = path
                    .normalized()
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .map(parts)
                    .collect();
                let specificity = segments
                    .iter()
                    .filter(|s| s.iter().all(|p| matches!(p, Part::Literal(_))))
                    .count();
                Some(Route {
                    method: method.to_string(),
                    segments,
                    identity: op.key.identity(),
                    specificity,
                })
            }
            OperationKey::Rpc { .. } => None,
        })
        .collect();
    routes.sort_by_key(|r| std::cmp::Reverse(r.specificity));
    for route in &routes {
        index
            .operations
            .insert(route.identity.clone(), OperationUsage::default());
    }

    for line in lines {
        let line = line.as_ref();
        let Some((method, target)) = request(line) else {
            if !line.trim().is_empty() {
                index.unmatched += 1;
            }
            continue;
        };
        let (path, query) = split_target(&target);
        let path = match &options.base_path {
            Some(base) => {
                let base = base.trim_end_matches('/');
                match path.strip_prefix(base) {
                    Some(rest) if rest.is_empty() || rest.starts_with('/') => rest.to_owned(),
                    _ => {
                        index.unmatched += 1;
                        continue;
                    }
                }
            }
            None => path,
        };
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let route = routes.iter().find(|r| {
            r.method.eq_ignore_ascii_case(&method)
                && r.segments.len() == segments.len()
                && r.segments.iter().zip(&segments).all(|(t, s)| fits(t, s))
        });
        let Some(route) = route else {
            index.unmatched += 1;
            continue;
        };
        let usage = index
            .operations
            .get_mut(&route.identity)
            .expect("every route is seeded");
        usage.calls += 1;
        for name in query_names(query) {
            *usage.parameters.entry(format!("query:{name}")).or_default() += 1;
        }
    }
    index
}

/// A template segment split into literal text and `{}` holes.
fn parts(segment: &str) -> Vec<Part> {
    let mut out = Vec::new();
    let mut literal = String::new();
    let mut rest = segment;
    while let Some(start) = rest.find("{}") {
        literal.push_str(&rest[..start]);
        if !literal.is_empty() {
            out.push(Part::Literal(std::mem::take(&mut literal)));
        }
        out.push(Part::Hole);
        rest = &rest[start + 2..];
    }
    literal.push_str(rest);
    if !literal.is_empty() {
        out.push(Part::Literal(literal));
    }
    out
}

/// Whether a request path segment fits a template segment: holes match
/// one or more characters.
fn fits(template: &[Part], segment: &str) -> bool {
    match template.split_first() {
        None => segment.is_empty(),
        Some((Part::Literal(text), rest)) => segment
            .strip_prefix(text.as_str())
            .is_some_and(|remaining| fits(rest, remaining)),
        Some((Part::Hole, rest)) => {
            // Try every split, shortest hole first.
            segment
                .char_indices()
                .skip(1)
                .map(|(i, _)| i)
                .chain(std::iter::once(segment.len()))
                .any(|end| end > 0 && fits(rest, &segment[end..]))
        }
    }
}

/// The method and request target of a log line.
fn request(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        if let Some(request) = value.get("request").and_then(|r| r.as_str()) {
            return request_line(request);
        }
        let method = value.get("method")?.as_str()?.to_owned();
        let target = ["path", "url", "uri", "request_uri"]
            .iter()
            .find_map(|key| value.get(*key).and_then(|v| v.as_str()))?
            .to_owned();
        return Some((method, target));
    }
    // Common/Combined Log Format: the request line is the first quoted
    // field.
    let start = line.find('"')? + 1;
    let end = start + line[start..].find('"')?;
    request_line(&line[start..end])
}

fn request_line(text: &str) -> Option<(String, String)> {
    let mut parts = text.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    method
        .bytes()
        .all(|b| b.is_ascii_alphabetic())
        .then(|| (method.to_owned(), target.to_owned()))
}

/// The path of a request target (origin or absolute form) and its query.
fn split_target(target: &str) -> (String, &str) {
    let without_fragment = target.split('#').next().unwrap_or(target);
    let (before, query) = without_fragment
        .split_once('?')
        .unwrap_or((without_fragment, ""));
    let path = match before.find("://") {
        Some(scheme_end) => {
            let after = &before[scheme_end + 3..];
            after.find('/').map_or("/", |slash| &after[slash..])
        }
        None => before,
    };
    (path.to_owned(), query)
}

/// Parameter names in a query string, without their values.
fn query_names(query: &str) -> Vec<String> {
    let mut names: Vec<String> = query
        .split('&')
        .filter_map(|pair| pair.split('=').next())
        .filter(|name| !name.is_empty())
        .map(|name| name.replace('+', " "))
        .collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_lines() {
        assert_eq!(
            request(
                r#"10.0.0.1 - - [10/Oct/2026:13:55:36 +0000] "GET /pets?limit=5 HTTP/1.1" 200 12"#
            ),
            Some(("GET".into(), "/pets?limit=5".into()))
        );
        assert_eq!(
            request(r#"{"method": "DELETE", "path": "/pets/7"}"#),
            Some(("DELETE".into(), "/pets/7".into()))
        );
        assert_eq!(
            request(r#"{"request": "POST /pets HTTP/2"}"#),
            Some(("POST".into(), "/pets".into()))
        );
        assert_eq!(request("garbage"), None);
        assert_eq!(request(r#"x "-" 400"#), None);
    }

    #[test]
    fn targets_and_queries() {
        assert_eq!(
            split_target("https://api.example.com/v1/pets?a=1#x"),
            ("/v1/pets".into(), "a=1")
        );
        assert_eq!(split_target("/pets"), ("/pets".into(), ""));
        assert_eq!(query_names("b=2&a=1&a=3&flag"), ["a", "b", "flag"]);
    }

    #[test]
    fn template_segments() {
        assert!(fits(&parts("{}"), "42"));
        assert!(!fits(&parts("{}"), ""));
        assert!(fits(&parts("{}.{}"), "report.pdf"));
        assert!(!fits(&parts("{}.{}"), "report"));
        assert!(fits(&parts("pets"), "pets"));
        assert!(!fits(&parts("pets"), "pet"));
    }
}
