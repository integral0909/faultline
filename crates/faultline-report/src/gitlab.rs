//! GitLab Code Quality reports: the subset of the Code Climate issue format
//! GitLab reads to show findings in merge requests.
//!
//! Each issue needs a file and line, so input errors without a span are
//! left out, and a version violation is placed at the start of a file of
//! the API it concerns. Waived and baselined findings are left out, like
//! in the other CI formats. Fingerprints are the finding ids, which stay
//! the same while a finding does, so GitLab can tell new issues from
//! resolved ones.

use std::fmt::Write as _;

use faultline_policy::Status;
use faultline_rules::{Severity, finding_id};
use serde_json::{Value, json};

use crate::location::Location;
use crate::{Report, short_witness};

pub(crate) fn render(report: &Report<'_>) -> String {
    let mut issues = Vec::new();
    for diagnostic in report.diagnostics.iter().filter(|d| d.is_error()) {
        let Some(location) = diagnostic
            .primary_span()
            .and_then(|s| Location::resolve(report.sources, s))
        else {
            continue;
        };
        let code = diagnostic.code.map(|c| c.to_string()).unwrap_or_default();
        let fingerprint = finding_id(
            "",
            &code,
            &format!("{}:{}", location.file, location.line),
            &diagnostic.message,
        );
        issues.push(issue(
            &diagnostic.message,
            if code.is_empty() {
                "input-error"
            } else {
                &code
            },
            &fingerprint,
            "blocker",
            &location,
        ));
    }
    let active = || {
        report
            .evaluation
            .findings
            .iter()
            .filter(|e| e.status == Status::Active)
            .map(|e| &e.finding)
    };
    for violation in &report.evaluation.version_violations {
        let Some(mut location) = active()
            .filter(|f| f.api == violation.api)
            .find_map(|f| Location::for_annotation(report.sources, &[f.change.new_span]))
        else {
            continue;
        };
        location.line = 1;
        location.end_line = 1;
        let message = crate::version_message(violation);
        let fingerprint = finding_id(&violation.api, "version", "", &message);
        issues.push(issue(
            &message,
            "version-bump",
            &fingerprint,
            "critical",
            &location,
        ));
    }
    for f in active() {
        let Some(location) =
            Location::for_annotation(report.sources, &[f.change.new_span, f.change.old_span])
        else {
            continue;
        };
        let severity = match f.severity {
            Severity::Breaking => "critical",
            Severity::Risky => "major",
            Severity::Info => "info",
        };
        let mut description = format!("{}: {}", f.change.path, f.message);
        if let Some(w) = f.witness() {
            let _ = write!(description, " (example: {})", short_witness(w));
        }
        issues.push(issue(&description, f.rule.code, &f.id, severity, &location));
    }
    let mut out =
        serde_json::to_string_pretty(&Value::Array(issues)).expect("JSON values serialize");
    out.push('\n');
    out
}

fn issue(
    description: &str,
    check: &str,
    fingerprint: &str,
    severity: &str,
    location: &Location,
) -> Value {
    json!({
        "type": "issue",
        "description": description,
        "check_name": check,
        "fingerprint": fingerprint,
        "severity": severity,
        "categories": ["Compatibility"],
        "location": {
            "path": location.file,
            "lines": {"begin": location.line, "end": location.end_line},
        },
    })
}
