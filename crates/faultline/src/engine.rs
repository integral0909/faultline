use std::path::{Path, PathBuf};

use faultline_doc::loader::{FsLoader, LoadError};
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, Role};
use faultline_jsonschema::Options;
use faultline_policy::{ApiFormat, SchemaRole};
use faultline_report::ApiSummary;
use faultline_rules::{Finding, classify, suggest_bump};
use faultline_source::{Diagnostic, Diagnostics, Limits, SourceMap};

use crate::codes;

/// Where to find one version of an API: an entry document inside a workspace
/// root. References may reach any file under the root and nothing outside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub root: PathBuf,
    /// Relative to `root`, or absolute inside it.
    pub entry: PathBuf,
}

impl Source {
    pub fn new(root: impl Into<PathBuf>, entry: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            entry: entry.into(),
        }
    }
}

/// One API to compare between two versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub old: Source,
    pub new: Source,
    /// Detected from the entry document when `None`.
    pub format: Option<ApiFormat>,
    /// For standalone JSON Schemas: which direction documents flow.
    pub role: SchemaRole,
}

impl Target {
    pub fn new(name: impl Into<String>, old: Source, new: Source) -> Self {
        Self {
            name: name.into(),
            old,
            new,
            format: None,
            role: SchemaRole::Both,
        }
    }
}

/// The lowered form of one version.
#[derive(Debug)]
pub struct Lowered {
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
}

/// Everything a comparison produced. Spans in diagnostics and findings refer
/// to `sources`.
#[derive(Debug, Default)]
pub struct Comparison {
    pub sources: SourceMap,
    pub diagnostics: Diagnostics,
    pub apis: Vec<ApiSummary>,
    pub findings: Vec<Finding>,
}

/// Loads, lowers and compares APIs.
#[derive(Clone, Debug, Default)]
pub struct Engine {
    pub limits: Limits,
}

impl Engine {
    /// Compares every target. Targets whose versions cannot be lowered
    /// without errors contribute diagnostics and no findings.
    pub fn compare(&self, targets: &[Target]) -> Comparison {
        let mut comparison = Comparison::default();
        for target in targets {
            let old = self.lower(
                &target.old,
                &target.name,
                target.format,
                target.role,
                &comparison.sources,
            );
            let new = self.lower(
                &target.new,
                &target.name,
                target.format,
                target.role,
                &comparison.sources,
            );
            let clean = !old.diagnostics.has_errors() && !new.diagnostics.has_errors();
            comparison.diagnostics.extend(old.diagnostics);
            comparison.diagnostics.extend(new.diagnostics);
            let (Some(old), Some(new), true) = (old.api, new.api, clean) else {
                continue;
            };
            let changes = faultline_diff::diff(&old, &new);
            let findings: Vec<Finding> =
                changes.iter().map(|c| classify(&target.name, c)).collect();
            comparison.apis.push(ApiSummary {
                name: target.name.clone(),
                old_version: old.version.clone(),
                new_version: new.version.clone(),
                bump: suggest_bump(&findings),
            });
            comparison.findings.extend(findings);
        }
        comparison.diagnostics.sort();
        comparison
    }

    /// Lowers one version of an API, registering its files in `sources`.
    pub fn lower(
        &self,
        source: &Source,
        name: &str,
        format: Option<ApiFormat>,
        role: SchemaRole,
        sources: &SourceMap,
    ) -> Lowered {
        let failure = |code, message: String| Lowered {
            api: None,
            diagnostics: [Diagnostic::error(message).with_code(code)]
                .into_iter()
                .collect(),
        };
        let loader = match FsLoader::new(&source.root, self.limits.clone()) {
            Ok(loader) => loader,
            Err(e) => {
                return failure(
                    codes::INVALID_ROOT,
                    format!(
                        "cannot use {} as the workspace root: {e}",
                        source.root.display()
                    ),
                );
            }
        };
        let entry = match loader.uri_for(&source.entry) {
            Ok(uri) => uri,
            Err(LoadError::OutsideRoot) => {
                return failure(
                    codes::ENTRY_NOT_FOUND,
                    format!(
                        "{} is outside the workspace root {}",
                        source.entry.display(),
                        source.root.display()
                    ),
                );
            }
            Err(e) => {
                return failure(
                    codes::ENTRY_NOT_FOUND,
                    format!("cannot read {}: {e}", source.entry.display()),
                );
            }
        };
        let mut store = DocumentStore::new(&loader, sources, self.limits.clone());
        let format = match format {
            Some(format) => format,
            None => match detect_format(&mut store, &entry) {
                Some(format) => format,
                // The document failed to load or parse; lowering reports why.
                None => ApiFormat::JsonSchema,
            },
        };
        let lowered = match format {
            ApiFormat::OpenApi => faultline_openapi::lower_openapi(&entry, &mut store, name),
            ApiFormat::JsonSchema => faultline_jsonschema::lower_document(
                &entry,
                &mut store,
                Options::default(),
                ir_role(role),
            ),
        };
        let mut api = lowered.api;
        if let Some(api) = &mut api {
            name.clone_into(&mut api.name);
        }
        Lowered {
            api,
            diagnostics: lowered.diagnostics,
        }
    }
}

/// Recognizes OpenAPI (and Swagger, so the OpenAPI front end can say it is
/// unsupported) by its version field; everything else is JSON Schema.
pub fn detect_format(store: &mut DocumentStore<'_>, entry: &UriRef) -> Option<ApiFormat> {
    let document = store.get(entry).ok()?;
    let root = &document.document.root;
    if root.get("openapi").is_some() || root.get("swagger").is_some() {
        Some(ApiFormat::OpenApi)
    } else {
        Some(ApiFormat::JsonSchema)
    }
}

fn ir_role(role: SchemaRole) -> Role {
    match role {
        SchemaRole::Input => Role::Input,
        SchemaRole::Output => Role::Output,
        SchemaRole::Both => Role::Both,
    }
}

/// The API name used when none is configured: the entry file's stem.
pub fn default_api_name(path: &Path) -> String {
    path.file_stem()
        .map_or_else(|| "api".to_owned(), |s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::default_api_name as stem;
    use std::path::Path;

    #[test]
    fn stems() {
        assert_eq!(stem(Path::new("api/openapi.yaml")), "openapi");
        assert_eq!(stem(Path::new("/")), "api");
    }
}
