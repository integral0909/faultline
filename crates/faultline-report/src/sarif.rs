//! SARIF 2.1.0 output for code-scanning integrations.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use faultline_policy::{Evaluated, Status};
use faultline_rules::{Rule, Severity};
use serde_json::{Value, json};

use crate::location::Location;
use crate::{Report, TOOL, TOOL_URI, TOOL_VERSION, short_witness};

pub(crate) fn render(report: &Report<'_>) -> String {
    let findings = &report.evaluation.findings;
    let mut rules: BTreeMap<&str, &Rule> = BTreeMap::new();
    for e in findings {
        rules.insert(e.finding.rule.code, e.finding.rule);
    }
    let index: BTreeMap<&str, usize> = rules
        .keys()
        .enumerate()
        .map(|(i, code)| (*code, i))
        .collect();

    let document = json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": TOOL,
                    "version": TOOL_VERSION,
                    "informationUri": TOOL_URI,
                    "rules": rules.values().map(|r| rule(r)).collect::<Vec<_>>(),
                }
            },
            "columnKind": "utf16CodeUnits",
            "results": findings.iter().map(|e| result(report, e, index[e.finding.rule.code])).collect::<Vec<_>>(),
        }]
    });
    let mut text = serde_json::to_string_pretty(&document).expect("reports always serialize");
    text.push('\n');
    text
}

fn rule(r: &Rule) -> Value {
    json!({
        "id": r.code,
        "name": r.name,
        "shortDescription": { "text": r.title },
        "fullDescription": { "text": r.explanation },
        "help": { "text": r.explanation, "markdown": r.explanation },
        "properties": { "tags": [r.dimension.as_str()] },
    })
}

fn result(report: &Report<'_>, e: &Evaluated, rule_index: usize) -> Value {
    let f = &e.finding;
    let level = match f.severity {
        Severity::Breaking => "error",
        Severity::Risky => "warning",
        Severity::Info => "note",
    };
    let mut message = format!("{}: {}", f.change.path, f.message);
    if let Some(w) = f.witness() {
        let _ = write!(message, " (example: {})", short_witness(w));
    }
    let spans = [f.change.new_span, f.change.old_span];
    let physical = Location::for_annotation(report.sources, &spans).map(|l| {
        json!({
            "physicalLocation": {
                "artifactLocation": { "uri": l.file },
                "region": {
                    "startLine": l.line,
                    "startColumn": l.utf16_column,
                    "endLine": l.end_line,
                    "endColumn": l.utf16_end_column,
                }
            },
            "logicalLocations": [{ "fullyQualifiedName": f.change.path.to_string(), "kind": "member" }],
        })
    });
    let mut result = json!({
        "ruleId": f.rule.code,
        "ruleIndex": rule_index,
        "level": level,
        "message": { "text": message },
        "partialFingerprints": { "faultlineFindingId/v1": f.id },
        "locations": physical.map_or_else(Vec::new, |p| vec![p]),
        "properties": { "api": f.api, "severity": f.severity.as_str(), "polarity": f.change.polarity.as_str() },
    });
    let suppression = match &e.status {
        Status::Active => None,
        Status::Waived(w) => Some(json!({"kind": "external", "justification": w.reason})),
        Status::Baselined => Some(json!({"kind": "external", "justification": "in baseline"})),
    };
    if let Some(suppression) = suppression {
        result["suppressions"] = json!([suppression]);
    }
    result
}
