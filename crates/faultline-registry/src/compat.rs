//! Checking a new version against the registered ones.

use faultline::{Engine, Lowered, compare_apis};
use faultline_doc::loader::MemoryLoader;
use faultline_doc::loader::workspace_uri;
use faultline_policy::{Config, Date, evaluate};
use faultline_report::{ApiSummary, Report, json_value};
use faultline_source::{Diagnostics, SourceMap};
use serde_json::Value;

use crate::store::{Bundle, Compatibility, Record};

/// The outcome of a check.
#[derive(Debug)]
pub struct Verdict {
    /// The new version lowered without errors.
    pub valid: bool,
    /// No failing finding against any version it was compared with.
    pub compatible: bool,
    /// The version the new description declares.
    pub api_version: Option<String>,
    /// The versions compared with.
    pub compared_with: Vec<u32>,
    /// The standard JSON report.
    pub report: Value,
}

/// Lowers a bundle, reading its files from memory only: a registry never
/// follows references outside what it was sent.
fn lower(engine: &Engine, bundle: &Bundle, name: &str, sources: &SourceMap) -> Lowered {
    let mut loader = MemoryLoader::new();
    for (path, text) in &bundle.files {
        loader.insert(
            &workspace_uri(std::path::Path::new(path)).to_string(),
            text.clone(),
        );
    }
    let entry = workspace_uri(std::path::Path::new(&bundle.entry));
    engine.lower_entry(&loader, &entry, name, bundle.format, bundle.role, sources)
}

/// Checks `new` against `history` (ascending) under `settings`: against
/// the latest version, or every version when transitive.
pub fn check(
    subject: &str,
    new: &Bundle,
    history: &[Record],
    settings: Compatibility,
    today: Date,
) -> Verdict {
    let engine = Engine::default();
    let sources = SourceMap::new();
    let mut diagnostics = Diagnostics::new();
    let lowered = lower(&engine, new, subject, &sources);
    let valid = lowered.api.is_some() && !lowered.diagnostics.has_errors();
    diagnostics.extend(lowered.diagnostics);
    let api_version = lowered.api.as_ref().and_then(|api| api.version.clone());

    let compared: Vec<&Record> = match (settings.transitive, history.last()) {
        (_, None) => Vec::new(),
        (true, Some(_)) => history.iter().collect(),
        (false, Some(latest)) => vec![latest],
    };
    let mut findings = Vec::new();
    let mut apis: Vec<ApiSummary> = Vec::new();
    let mut compared_with = Vec::new();
    if let (true, Some(new_api)) = (valid, &lowered.api) {
        for record in &compared {
            // Findings name the subject, so waivers and baselines kept by
            // clients match; comparisons with older versions say which.
            let name = if Some(record.version) == history.last().map(|r| r.version) {
                subject.to_owned()
            } else {
                format!("{subject}@v{}", record.version)
            };
            let old = lower(&engine, &record.bundle, &name, &sources);
            let Some(old_api) = old.api.filter(|_| !old.diagnostics.has_errors()) else {
                // A version that lowered when it was registered no longer
                // does (the tool changed): say so and skip it.
                diagnostics.extend(old.diagnostics);
                continue;
            };
            let (found, summary) = compare_apis(&name, &old_api, new_api);
            findings.extend(found);
            apis.push(summary);
            compared_with.push(record.version);
        }
    }
    let config = Config {
        fail_on: settings.fail_on,
        ..Config::default()
    };
    let evaluation = evaluate(&config, None, findings, today);
    diagnostics.sort_in(&sources);
    let report = json_value(&Report {
        sources: &sources,
        diagnostics: &diagnostics,
        apis: &apis,
        evaluation: &evaluation,
    });
    Verdict {
        valid,
        compatible: valid && evaluation.failing() == 0,
        api_version,
        compared_with,
        report,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faultline_policy::FailOn;
    use serde_json::json;

    const V1: &str = r#"{"$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object", "properties": {"id": {"type": "integer", "maximum": 100}}}"#;

    fn bundle(text: &str) -> Bundle {
        Bundle::from_json(&json!({
            "entry": "pet.json",
            "role": "input",
            "files": {"pet.json": text},
        }))
        .unwrap()
    }

    fn record(version: u32, text: &str) -> Record {
        Record {
            version,
            registered: "2026-09-28".into(),
            api_version: None,
            bundle: bundle(text),
        }
    }

    fn today() -> Date {
        Date::parse("2026-09-28").unwrap()
    }

    #[test]
    fn first_versions_only_need_to_be_valid() {
        let verdict = check("pet", &bundle(V1), &[], Compatibility::default(), today());
        assert!(verdict.valid && verdict.compatible);
        assert!(verdict.compared_with.is_empty());
        let broken = check("pet", &bundle("{"), &[], Compatibility::default(), today());
        assert!(!broken.valid && !broken.compatible);
        assert!(!broken.report["diagnostics"].as_array().unwrap().is_empty());
    }

    #[test]
    fn breaking_versions_are_refused() {
        let history = [record(1, V1)];
        let tighter = V1.replace("100", "50");
        let verdict = check(
            "pet",
            &bundle(&tighter),
            &history,
            Compatibility::default(),
            today(),
        );
        assert!(verdict.valid);
        assert!(!verdict.compatible);
        assert_eq!(verdict.compared_with, [1]);
        assert_eq!(verdict.report["summary"]["breaking"], 1);
        let lenient = Compatibility {
            fail_on: FailOn::Never,
            transitive: false,
        };
        assert!(check("pet", &bundle(&tighter), &history, lenient, today()).compatible);
        let looser = V1.replace("100", "500");
        assert!(
            check(
                "pet",
                &bundle(&looser),
                &history,
                Compatibility::default(),
                today()
            )
            .compatible
        );
    }

    #[test]
    fn transitive_checks_reach_every_version() {
        // v1 allows 100, v2 allowed 500; going back to 200 is fine against
        // v1 but breaks v2's clients, and 150 against v1 only matters when
        // transitive.
        let history = [record(1, V1), record(2, &V1.replace("100", "500"))];
        let new = bundle(&V1.replace("100", "200"));
        let latest_only = check("pet", &new, &history, Compatibility::default(), today());
        assert_eq!(latest_only.compared_with, [2]);
        assert!(!latest_only.compatible);
        let transitive = Compatibility {
            fail_on: FailOn::Breaking,
            transitive: true,
        };
        let all = check("pet", &new, &history, transitive, today());
        assert_eq!(all.compared_with, [1, 2]);
        let names: Vec<&str> = all.report["apis"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["pet@v1", "pet"]);
    }
}
