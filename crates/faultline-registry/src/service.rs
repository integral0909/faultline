//! The registry's HTTP interface.
//!
//! | Method | Path | |
//! |---|---|---|
//! | `GET` | `/health` | Liveness |
//! | `GET` | `/subjects` | Subjects with at least one version |
//! | `GET` | `/subjects/{s}/versions` | Version numbers |
//! | `POST` | `/subjects/{s}/versions` | Register a version (checked) |
//! | `GET` | `/subjects/{s}/versions/{v}` | A version, `latest` for the newest |
//! | `GET` | `/subjects/{s}/versions/{v}/files/{path}` | One file's text |
//! | `POST` | `/subjects/{s}/check` | Check a version without registering |
//! | `GET`, `PUT` | `/subjects/{s}/config` | Compatibility settings |
//!
//! Writes (`POST .../versions`, `PUT .../config`) require the bearer token
//! when one is configured. Request bodies are JSON and must say so, which
//! also keeps browsers from sending them across origins without asking.

use std::sync::Arc;

use faultline_policy::Date;
use serde_json::{Value, json};

use crate::compat::check;
use crate::http::{Handler, Request, Response};
use crate::store::{Admission, Bundle, Compatibility, Record, Store, StoreError};

/// A registry: storage, settings and the credentials writes need.
#[derive(Debug)]
pub struct Registry {
    store: Store,
    /// Settings for subjects that have none of their own.
    defaults: Compatibility,
    token: Option<String>,
    today: Option<Date>,
}

impl Registry {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            defaults: Compatibility::default(),
            token: None,
            today: None,
        }
    }

    #[must_use]
    pub fn with_defaults(mut self, defaults: Compatibility) -> Self {
        self.defaults = defaults;
        self
    }

    /// Requires `Authorization: Bearer <token>` on writes.
    #[must_use]
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token.filter(|t| !t.is_empty());
        self
    }

    /// Dates registrations `today` instead of by the clock.
    #[must_use]
    pub fn with_today(mut self, today: Date) -> Self {
        self.today = Some(today);
        self
    }

    fn today(&self) -> Date {
        self.today.unwrap_or_else(Date::today)
    }

    /// The registry as a handler for [`crate::http::Server`].
    pub fn into_handler(self) -> Arc<Handler> {
        let registry = Arc::new(self);
        Arc::new(move |request: &Request| registry.handle(request))
    }

    /// Answers one request.
    pub fn handle(&self, request: &Request) -> Response {
        let segments: Vec<&str> = request.segments.iter().map(String::as_str).collect();
        let method = request.method.as_str();
        let result = match (method, segments.as_slice()) {
            ("GET" | "HEAD", ["health"]) => Ok(Response::json(200, &json!({"status": "ok"}))),
            ("GET" | "HEAD", ["subjects"]) => self.list_subjects(),
            ("GET" | "HEAD", ["subjects", subject, "versions"]) => self.list_versions(subject),
            ("POST", ["subjects", subject, "versions"]) => self
                .authorize(request)
                .and_then(|()| self.register(subject, request)),
            ("GET" | "HEAD", ["subjects", subject, "versions", version]) => {
                self.version(subject, version)
            }
            ("GET" | "HEAD", ["subjects", subject, "versions", version, "files", path @ ..])
                if !path.is_empty() =>
            {
                self.file(subject, version, &path.join("/"))
            }
            ("POST", ["subjects", subject, "check"]) => self.check_only(subject, request),
            ("GET" | "HEAD", ["subjects", subject, "config"]) => self.config(subject),
            ("PUT", ["subjects", subject, "config"]) => self
                .authorize(request)
                .and_then(|()| self.set_config(subject, request)),
            (_, ["health" | "subjects"] | ["subjects", _, "versions" | "check" | "config", ..]) => {
                Err(Response::error(
                    405,
                    format!("{method} is not allowed here"),
                ))
            }
            _ => Err(Response::error(404, "no such resource")),
        };
        let response = result.unwrap_or_else(|response| response);
        if method == "HEAD" {
            Response {
                body: Vec::new(),
                ..response
            }
        } else {
            response
        }
    }

    fn authorize(&self, request: &Request) -> Result<(), Response> {
        let Some(token) = &self.token else {
            return Ok(());
        };
        let presented = request
            .header("authorization")
            .and_then(|value| value.strip_prefix("Bearer "));
        match presented {
            Some(presented) if constant_time_eq(presented.as_bytes(), token.as_bytes()) => Ok(()),
            Some(_) => Err(Response::error(403, "the token is not valid")),
            None => Err(Response::error(401, "this request needs a bearer token")
                .with_header("WWW-Authenticate", "Bearer")),
        }
    }

    fn list_subjects(&self) -> Result<Response, Response> {
        let subjects = self.store.subjects().map_err(store_error)?;
        Ok(Response::json(200, &json!({"subjects": subjects})))
    }

    fn list_versions(&self, subject: &str) -> Result<Response, Response> {
        let versions = self.store.versions(subject).map_err(store_error)?;
        if versions.is_empty() {
            return Err(Response::error(404, format!("no subject `{subject}`")));
        }
        Ok(Response::json(
            200,
            &json!({"subject": subject, "versions": versions}),
        ))
    }

    fn record(&self, subject: &str, version: &str) -> Result<Record, Response> {
        let number = if version == "latest" {
            *self
                .store
                .versions(subject)
                .map_err(store_error)?
                .last()
                .ok_or_else(|| Response::error(404, format!("no subject `{subject}`")))?
        } else {
            version
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0 && !version.starts_with('0'))
                .ok_or_else(|| Response::error(400, "a version is a positive number or `latest`"))?
        };
        self.store.get(subject, number).map_err(store_error)
    }

    fn version(&self, subject: &str, version: &str) -> Result<Response, Response> {
        let record = self.record(subject, version)?;
        let mut value = record.to_json();
        value["subject"] = json!(subject);
        if let Some(bundle) = value.get("bundle") {
            value["files"] = json!(
                bundle["files"]
                    .as_object()
                    .map(|f| f.keys().collect::<Vec<_>>())
            );
        }
        Ok(Response::json(200, &value))
    }

    fn file(&self, subject: &str, version: &str, path: &str) -> Result<Response, Response> {
        let record = self.record(subject, version)?;
        let text = record.bundle.files.get(path).ok_or_else(|| {
            Response::error(
                404,
                format!("version {} has no file `{path}`", record.version),
            )
        })?;
        let content_type = match path.rsplit('.').next() {
            Some("json" | "avsc") => "application/json",
            Some("yaml" | "yml") => "application/yaml",
            _ => "text/plain; charset=utf-8",
        };
        Ok(Response {
            content_type,
            ..Response::text(200, text.clone())
        })
    }

    fn check_only(&self, subject: &str, request: &Request) -> Result<Response, Response> {
        let bundle = bundle_from(request)?;
        let settings = self
            .store
            .compatibility(subject, self.defaults)
            .map_err(store_error)?;
        let history = self
            .store
            .versions(subject)
            .map_err(store_error)?
            .into_iter()
            .map(|v| self.store.get(subject, v))
            .collect::<Result<Vec<_>, _>>()
            .map_err(store_error)?;
        let verdict = check(subject, &bundle, &history, settings, self.today());
        Ok(Response::json(
            200,
            &json!({
                "subject": subject,
                "valid": verdict.valid,
                "compatible": verdict.compatible,
                "compared_with": verdict.compared_with,
                "report": verdict.report,
            }),
        ))
    }

    fn register(&self, subject: &str, request: &Request) -> Result<Response, Response> {
        let bundle = bundle_from(request)?;
        let settings = self
            .store
            .compatibility(subject, self.defaults)
            .map_err(store_error)?;
        let today = self.today();
        let mut compared_with = Vec::new();
        let mut report = Value::Null;
        let outcome = self
            .store
            .register(subject, &today.to_string(), |history| {
                let verdict = check(subject, &bundle, history, settings, today);
                compared_with.clone_from(&verdict.compared_with);
                report = verdict.report;
                if !verdict.valid {
                    return Err(Response::json(
                        422,
                        &json!({
                            "error": "the description has errors",
                            "subject": subject,
                            "report": report,
                        }),
                    ));
                }
                if !verdict.compatible {
                    return Err(Response::json(
                        409,
                        &json!({
                            "error": format!(
                                "the version is incompatible with {}",
                                describe_versions(&verdict.compared_with)
                            ),
                            "subject": subject,
                            "compared_with": verdict.compared_with,
                            "report": report,
                        }),
                    ));
                }
                Ok(Admission {
                    bundle: bundle.clone(),
                    api_version: verdict.api_version,
                })
            })
            .map_err(store_error)??;
        let version = outcome.record.version;
        let body = json!({
            "subject": subject,
            "version": version,
            "created": outcome.created,
            "compared_with": compared_with,
            "report": report,
        });
        let status = if outcome.created { 201 } else { 200 };
        Ok(Response::json(status, &body).with_header(
            "Location",
            format!("/subjects/{subject}/versions/{version}"),
        ))
    }

    fn config(&self, subject: &str) -> Result<Response, Response> {
        let settings = self
            .store
            .compatibility(subject, self.defaults)
            .map_err(store_error)?;
        let mut value = settings.to_json();
        value["subject"] = json!(subject);
        Ok(Response::json(200, &value))
    }

    fn set_config(&self, subject: &str, request: &Request) -> Result<Response, Response> {
        let value = json_body(request)?;
        let current = self
            .store
            .compatibility(subject, self.defaults)
            .map_err(store_error)?;
        let settings =
            Compatibility::from_json(&value, current).map_err(|e| Response::error(400, e))?;
        self.store
            .set_compatibility(subject, settings)
            .map_err(store_error)?;
        let mut value = settings.to_json();
        value["subject"] = json!(subject);
        Ok(Response::json(200, &value))
    }
}

fn describe_versions(versions: &[u32]) -> String {
    match versions {
        [one] => format!("version {one}"),
        many => format!(
            "versions {}",
            many.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn json_body(request: &Request) -> Result<Value, Response> {
    let content_type = request.header("content-type").unwrap_or_default();
    let media_type = content_type.split(';').next().unwrap_or_default().trim();
    if !media_type.eq_ignore_ascii_case("application/json") {
        return Err(Response::error(415, "the body must be application/json"));
    }
    serde_json::from_slice(&request.body)
        .map_err(|e| Response::error(400, format!("the body is not valid JSON: {e}")))
}

fn bundle_from(request: &Request) -> Result<Bundle, Response> {
    Bundle::from_json(&json_body(request)?).map_err(|e| Response::error(400, e))
}

fn store_error(error: StoreError) -> Response {
    match error {
        StoreError::Invalid(message) => Response::error(400, message),
        StoreError::NotFound(message) => Response::error(404, message),
        StoreError::Conflict(message) => Response::error(409, message),
        // Storage details stay in the server's log.
        StoreError::Io(_) | StoreError::Corrupt(_) => {
            eprintln!("faultline registry: {error}");
            Response::error(500, "the registry could not read or write its storage")
        }
    }
}

/// Compares secrets in time independent of where they differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const V1: &str = r#"{"$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object", "properties": {"id": {"type": "integer", "maximum": 100}}}"#;

    fn registry(dir: &std::path::Path) -> Registry {
        Registry::new(Store::open(dir).unwrap())
            .with_token(Some("placeholder-token".into()))
            .with_today(Date::parse("2026-09-28").unwrap())
    }

    fn request(method: &str, path: &str, body: Option<Value>, token: bool) -> Request {
        let mut headers = BTreeMap::new();
        if body.is_some() {
            headers.insert("content-type".into(), "application/json".into());
        }
        if token {
            headers.insert("authorization".into(), "Bearer placeholder-token".into());
        }
        Request {
            method: method.into(),
            segments: path
                .split('/')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
            query: BTreeMap::new(),
            headers,
            body: body.map(|b| b.to_string().into_bytes()).unwrap_or_default(),
        }
    }

    fn bundle(text: &str) -> Value {
        json!({"entry": "pet.json", "role": "input", "files": {"pet.json": text}})
    }

    fn body(response: &Response) -> Value {
        serde_json::from_slice(&response.body).unwrap()
    }

    #[test]
    fn registration_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path());
        let send = |method: &str, path: &str, b: Option<Value>| {
            registry.handle(&request(method, path, b, true))
        };

        let created = send("POST", "/subjects/pet/versions", Some(bundle(V1)));
        assert_eq!(created.status, 201, "{:?}", body(&created));
        assert_eq!(body(&created)["version"], 1);
        assert!(
            created
                .headers
                .iter()
                .any(|(n, v)| *n == "Location" && v == "/subjects/pet/versions/1")
        );

        let again = send("POST", "/subjects/pet/versions", Some(bundle(V1)));
        assert_eq!(again.status, 200);
        assert_eq!(body(&again)["created"], false);

        let breaking = send(
            "POST",
            "/subjects/pet/versions",
            Some(bundle(&V1.replace("100", "50"))),
        );
        assert_eq!(breaking.status, 409);
        assert_eq!(body(&breaking)["report"]["summary"]["breaking"], 1);
        assert!(
            body(&breaking)["error"]
                .as_str()
                .unwrap()
                .contains("version 1")
        );

        let dry_run = send(
            "POST",
            "/subjects/pet/check",
            Some(bundle(&V1.replace("100", "50"))),
        );
        assert_eq!(dry_run.status, 200);
        assert_eq!(body(&dry_run)["compatible"], false);

        let invalid = send("POST", "/subjects/pet/versions", Some(bundle("{")));
        assert_eq!(invalid.status, 422);

        let looser = send(
            "POST",
            "/subjects/pet/versions",
            Some(bundle(&V1.replace("100", "500"))),
        );
        assert_eq!(looser.status, 201);
        assert_eq!(body(&looser)["version"], 2);

        assert_eq!(
            body(&send("GET", "/subjects", None))["subjects"],
            json!(["pet"])
        );
        assert_eq!(
            body(&send("GET", "/subjects/pet/versions", None))["versions"],
            json!([1, 2])
        );
        let latest = body(&send("GET", "/subjects/pet/versions/latest", None));
        assert_eq!(latest["version"], 2);
        assert_eq!(latest["registered"], "2026-09-28");
        assert_eq!(latest["files"], json!(["pet.json"]));
        let file = send("GET", "/subjects/pet/versions/1/files/pet.json", None);
        assert_eq!(file.status, 200);
        assert_eq!(file.content_type, "application/json");
        assert_eq!(String::from_utf8(file.body).unwrap(), V1);

        for (path, status) in [
            ("/subjects/pet/versions/3", 404),
            ("/subjects/pet/versions/0", 400),
            ("/subjects/pet/versions/01", 400),
            ("/subjects/pet/versions/1/files/nope.json", 404),
            ("/subjects/other/versions", 404),
            ("/subjects/.hidden/versions", 400),
            ("/nothing", 404),
        ] {
            assert_eq!(send("GET", path, None).status, status, "{path}");
        }
        assert_eq!(send("DELETE", "/subjects/pet/versions/1", None).status, 405);
        let head = send("HEAD", "/subjects/pet/versions/1", None);
        assert_eq!(head.status, 200);
        assert!(head.body.is_empty());
    }

    #[test]
    fn writes_need_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path());
        let anonymous = registry.handle(&request(
            "POST",
            "/subjects/pet/versions",
            Some(bundle(V1)),
            false,
        ));
        assert_eq!(anonymous.status, 401);
        let mut wrong = request("PUT", "/subjects/pet/config", Some(json!({})), false);
        wrong
            .headers
            .insert("authorization".into(), "Bearer another-token".into());
        assert_eq!(registry.handle(&wrong).status, 403);
        // Reads and dry runs are open.
        assert_eq!(
            registry
                .handle(&request("GET", "/subjects", None, false))
                .status,
            200
        );
        assert_eq!(
            registry
                .handle(&request(
                    "POST",
                    "/subjects/pet/check",
                    Some(bundle(V1)),
                    false
                ))
                .status,
            200
        );
        let open = Registry::new(Store::open(dir.path()).unwrap());
        assert_eq!(
            open.handle(&request(
                "POST",
                "/subjects/pet/versions",
                Some(bundle(V1)),
                false
            ))
            .status,
            201
        );
    }

    #[test]
    fn bodies_must_be_json() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path());
        let mut form = request("POST", "/subjects/pet/versions", Some(bundle(V1)), true);
        form.headers.insert(
            "content-type".into(),
            "application/x-www-form-urlencoded".into(),
        );
        assert_eq!(registry.handle(&form).status, 415);
        let mut garbage = request("POST", "/subjects/pet/versions", Some(json!(1)), true);
        garbage.body = b"{nope".to_vec();
        assert_eq!(registry.handle(&garbage).status, 400);
        let charset = {
            let mut r = request("POST", "/subjects/pet/versions", Some(bundle(V1)), true);
            r.headers.insert(
                "content-type".into(),
                "Application/JSON; charset=utf-8".into(),
            );
            r
        };
        assert_eq!(registry.handle(&charset).status, 201);
    }

    #[test]
    fn settings() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path());
        let get = registry.handle(&request("GET", "/subjects/pet/config", None, false));
        assert_eq!(
            body(&get),
            json!({"subject": "pet", "fail_on": "breaking", "transitive": false})
        );
        let put = registry.handle(&request(
            "PUT",
            "/subjects/pet/config",
            Some(json!({"fail_on": "never"})),
            true,
        ));
        assert_eq!(put.status, 200);
        assert_eq!(body(&put)["fail_on"], "never");
        let bad = registry.handle(&request(
            "PUT",
            "/subjects/pet/config",
            Some(json!({"fail_on": 1})),
            true,
        ));
        assert_eq!(bad.status, 400);
        // With failures turned off, a breaking version is admitted.
        registry.handle(&request(
            "POST",
            "/subjects/pet/versions",
            Some(bundle(V1)),
            true,
        ));
        let breaking = registry.handle(&request(
            "POST",
            "/subjects/pet/versions",
            Some(bundle(&V1.replace("100", "50"))),
            true,
        ));
        assert_eq!(breaking.status, 201);
    }

    #[test]
    fn secrets_compare_fully() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
