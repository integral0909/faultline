//! Importing HTTP Archives (HAR files).
//!
//! A HAR file, as browsers' developer tools and many proxies export it,
//! records complete requests. Each request is matched to an operation like
//! an access log line (see [`crate::access_log`]); besides the call, the
//! names of its query parameters, of the header parameters the operation
//! declares, and of the members of its JSON or form body are recorded, so
//! the index knows which request fields clients send. Values are never
//! stored, and neither are responses: what a server sent says nothing about
//! what clients read.

use std::collections::{BTreeSet, HashMap};

use faultline_ir::{Api, ParamLocation};
use serde_json::Value;

use crate::access_log::{AccessLogOptions, Routes, query_names, split_target};
use crate::index::{Source, UsageIndex};

/// Body members are recorded to this depth.
const MAX_DEPTH: usize = 32;
/// At most this many body paths are recorded per request.
const MAX_PATHS: usize = 10_000;

/// Imports the entries of the HAR document `text` against the HTTP
/// operations of `api`.
pub fn import(api: &Api, text: &str, options: &AccessLogOptions) -> Result<UsageIndex, String> {
    let document: Value = serde_json::from_str(text).map_err(|e| format!("not a HAR file: {e}"))?;
    let entries = document
        .pointer("/log/entries")
        .and_then(Value::as_array)
        .ok_or("not a HAR file: there is no log.entries array")?;
    let routes = Routes::new(api, options);
    let mut index = routes.seed(Source::Har);
    // Header parameters each operation declares, by lower-case name: other
    // headers (cookies, user agents) are not the API's.
    let declared_headers: HashMap<String, BTreeSet<String>> = api
        .operations
        .iter()
        .map(|op| {
            let headers = op
                .parameters
                .iter()
                .filter(|p| p.location == ParamLocation::Header)
                .map(faultline_ir::Parameter::identity_name)
                .collect();
            (op.key.identity(), headers)
        })
        .collect();

    for entry in entries {
        let request = &entry["request"];
        let (Some(method), Some(url)) = (request["method"].as_str(), request["url"].as_str())
        else {
            index.unmatched += 1;
            continue;
        };
        let (path, query) = split_target(url);
        let Some(identity) = routes.find(method, &path) else {
            index.unmatched += 1;
            continue;
        };
        let identity = identity.to_owned();
        let usage = index
            .operations
            .get_mut(&identity)
            .expect("every route is seeded");
        usage.calls += 1;

        // `queryString` lists the parameters; the URL has them too.
        let mut names: BTreeSet<String> = query_names(query).into_iter().collect();
        for parameter in request["queryString"].as_array().into_iter().flatten() {
            if let Some(name) = parameter["name"].as_str().filter(|n| !n.is_empty()) {
                names.insert(name.to_owned());
            }
        }
        for name in names {
            *usage.parameters.entry(format!("query:{name}")).or_default() += 1;
        }
        let declared = declared_headers.get(&identity);
        let mut headers = BTreeSet::new();
        for header in request["headers"].as_array().into_iter().flatten() {
            if let Some(name) = header["name"].as_str() {
                let name = name.to_ascii_lowercase();
                if declared.is_some_and(|d| d.contains(&name)) {
                    headers.insert(name);
                }
            }
        }
        for name in headers {
            *usage
                .parameters
                .entry(format!("header:{name}"))
                .or_default() += 1;
        }
        for path in body_paths(&request["postData"]) {
            *usage.fields.entry(path).or_default() += 1;
        }
    }
    Ok(index)
}

/// The member paths of a request body, as `()name` and `()name.inner`
/// chains (see [`crate::field_chain`]).
fn body_paths(post: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mime = post["mimeType"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if mime.contains("json") {
        if let Some(value) = post["text"]
            .as_str()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
        {
            collect(&value, "()", 0, &mut out);
        }
    } else if let Some(params) = post["params"].as_array() {
        for param in params {
            if let Some(name) = param["name"].as_str().filter(|n| !n.is_empty()) {
                out.insert(format!("(){name}"));
            }
        }
    } else if mime.contains("x-www-form-urlencoded")
        && let Some(text) = post["text"].as_str()
    {
        for name in query_names(text) {
            out.insert(format!("(){name}"));
        }
    }
    out
}

/// Records the members of `value` under `prefix`. Array items share their
/// array's path, as in field chains.
fn collect(value: &Value, prefix: &str, depth: usize, out: &mut BTreeSet<String>) {
    if depth > MAX_DEPTH || out.len() >= MAX_PATHS {
        return;
    }
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                if out.len() >= MAX_PATHS {
                    return;
                }
                let path = if prefix == "()" {
                    format!("(){name}")
                } else {
                    format!("{prefix}.{name}")
                };
                collect(member, &path, depth + 1, out);
                out.insert(path);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect(item, prefix, depth + 1, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paths(post: &Value) -> Vec<String> {
        body_paths(post).into_iter().collect()
    }

    #[test]
    fn json_bodies_record_member_paths() {
        let post = json!({
            "mimeType": "application/json; charset=utf-8",
            "text": r#"{"name": "Rex", "owner": {"email": "a@example.com"}, "tags": [{"label": "x"}]}"#,
        });
        assert_eq!(
            paths(&post),
            [
                "()name",
                "()owner",
                "()owner.email",
                "()tags",
                "()tags.label"
            ]
        );
    }

    #[test]
    fn forms_record_parameter_names() {
        let listed = json!({"mimeType": "multipart/form-data", "params": [{"name": "file", "fileName": "x.png"}, {"name": "note"}]});
        assert_eq!(paths(&listed), ["()file", "()note"]);
        let text =
            json!({"mimeType": "application/x-www-form-urlencoded", "text": "user=a&password=b"});
        assert_eq!(paths(&text), ["()password", "()user"]);
        assert!(paths(&json!({"mimeType": "text/plain", "text": "hi"})).is_empty());
        assert!(paths(&json!({"mimeType": "application/json", "text": "{"})).is_empty());
    }
}
