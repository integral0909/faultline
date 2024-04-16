//! The JSON report: Faultline's stable machine-readable output.
//!
//! The document carries `schema_version`; fields are only ever added within a
//! version. Witness values are embedded as JSON values, with numbers kept
//! exact.

use faultline_policy::{Evaluated, Status};
use faultline_rules::Severity;
use faultline_source::Span;
use serde_json::{Value, json};

use crate::location::Location;
use crate::{JSON_SCHEMA_VERSION, Report, TOOL, TOOL_VERSION};

pub(crate) fn render(report: &Report<'_>) -> String {
    let mut text =
        serde_json::to_string_pretty(&document(report)).expect("reports always serialize");
    text.push('\n');
    text
}

pub(crate) fn document(report: &Report<'_>) -> Value {
    let evaluation = report.evaluation;
    let count_status = |wanted: fn(&Status) -> bool| {
        evaluation
            .findings
            .iter()
            .filter(|e| wanted(&e.status))
            .count()
    };
    json!({
        "schema_version": JSON_SCHEMA_VERSION,
        "tool": { "name": TOOL, "version": TOOL_VERSION },
        "passed": evaluation.exit_code() == 0 && !report.diagnostics.has_errors(),
        "exit_code": exit_code(report),
        "fail_on": format!("{:?}", evaluation.fail_on).to_lowercase(),
        "summary": {
            "breaking": evaluation.count(Severity::Breaking),
            "risky": evaluation.count(Severity::Risky),
            "info": evaluation.count(Severity::Info),
            "waived": count_status(|s| matches!(s, Status::Waived(_))),
            "baselined": count_status(|s| *s == Status::Baselined),
            "failing": evaluation.failing(),
        },
        "apis": report.apis.iter().map(|api| json!({
            "name": api.name,
            "old_version": api.old_version,
            "new_version": api.new_version,
            "suggested_bump": api.bump.as_str(),
        })).collect::<Vec<_>>(),
        "findings": evaluation.findings.iter().map(|e| finding(report, e)).collect::<Vec<_>>(),
        "expired_waivers": evaluation.expired_waivers.iter().map(|w| json!({
            "finding": w.finding, "reason": w.reason, "owner": w.owner, "expires": w.expires.to_string(),
        })).collect::<Vec<_>>(),
        "unused_waivers": evaluation.unused_waivers.iter().map(|w| json!({
            "finding": w.finding, "reason": w.reason, "owner": w.owner, "expires": w.expires.to_string(),
        })).collect::<Vec<_>>(),
        "diagnostics": report.diagnostics.iter().map(|d| json!({
            "severity": d.severity.as_str(),
            "code": d.code.map(faultline_source::DiagnosticCode::as_str),
            "message": d.message,
            "location": location(report, d.primary_span()),
        })).collect::<Vec<_>>(),
    })
}

/// The exit code a run with this report ends with: input errors outrank
/// findings.
pub(crate) fn exit_code(report: &Report<'_>) -> i32 {
    if report.diagnostics.has_errors() {
        faultline_policy::exit::INVALID_INPUT
    } else {
        report.evaluation.exit_code()
    }
}

fn finding(report: &Report<'_>, e: &Evaluated) -> Value {
    let f = &e.finding;
    let status = match &e.status {
        Status::Active => json!({"kind": "active"}),
        Status::Waived(w) => {
            json!({"kind": "waived", "reason": w.reason, "owner": w.owner, "expires": w.expires.to_string()})
        }
        Status::Baselined => json!({"kind": "baselined"}),
    };
    json!({
        "id": f.id,
        "api": f.api,
        "rule": f.rule.code,
        "rule_name": f.rule.name,
        "dimension": f.rule.dimension.as_str(),
        "severity": f.severity.as_str(),
        "original_severity": e.original_severity.map(Severity::as_str),
        "failing": e.failing,
        "status": status,
        "message": f.message,
        "path": f.change.path.to_string(),
        "polarity": f.change.polarity.as_str(),
        "old_location": location(report, f.change.old_span),
        "new_location": location(report, f.change.new_span),
        "witness": f.witness().map(literal),
    })
}

fn location(report: &Report<'_>, span: Option<Span>) -> Value {
    span.and_then(|s| Location::resolve(report.sources, s)).map_or(Value::Null, |l| {
        json!({"file": l.file, "revision": l.revision, "line": l.line, "column": l.column, "end_line": l.end_line, "end_column": l.end_column})
    })
}

/// A literal as a JSON value. Literals print as JSON, so this round-trips
/// exactly (`serde_json` keeps number text with `arbitrary_precision`).
pub(crate) fn literal(value: &faultline_ir::Literal) -> Value {
    serde_json::from_str(&value.to_string()).unwrap_or(Value::Null)
}
