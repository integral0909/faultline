//! Markdown for pull-request comments.

use std::fmt::Write as _;

use faultline_policy::{Evaluated, Status};
use faultline_rules::Severity;

use crate::{Report, short_witness};

pub(crate) fn render(report: &Report<'_>) -> String {
    let evaluation = report.evaluation;
    let mut out = String::new();
    let failing = evaluation.failing();
    let headline = if report.diagnostics.has_errors() {
        "Faultline could not check the API: the inputs have errors".to_owned()
    } else if failing > 0 {
        format!(
            "Faultline found {failing} failing {}",
            if failing == 1 { "change" } else { "changes" }
        )
    } else {
        "Faultline found no breaking changes".to_owned()
    };
    let _ = writeln!(out, "### {headline}\n");

    if !report.apis.is_empty() {
        out.push_str(
            "| API | Breaking | Risky | Compatible | Suggested bump |\n|---|---:|---:|---:|---|\n",
        );
        for api in report.apis {
            let count = |severity| {
                evaluation
                    .findings
                    .iter()
                    .filter(|e| {
                        e.finding.api == api.name
                            && e.status == Status::Active
                            && e.finding.severity == severity
                    })
                    .count()
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} |",
                escape(&api.name),
                count(Severity::Breaking),
                count(Severity::Risky),
                count(Severity::Info),
                api.bump
            );
        }
        out.push('\n');
    }

    for diagnostic in report.diagnostics.iter().filter(|d| d.is_error()) {
        let _ = writeln!(out, "- **input error**: {}", escape(&diagnostic.message));
    }

    let serious: Vec<&Evaluated> = evaluation
        .findings
        .iter()
        .filter(|e| e.status == Status::Active && e.finding.severity > Severity::Info)
        .collect();
    if !serious.is_empty() {
        out.push_str("| Severity | Rule | Where | Change |\n|---|---|---|---|\n");
        for e in serious {
            let f = &e.finding;
            let mut change = escape(&f.message);
            if let Some(w) = f.witness() {
                let _ = write!(
                    change,
                    "<br>example: `{}`",
                    short_witness(w).replace('`', "'")
                );
            }
            let _ = writeln!(
                out,
                "| {} | {} | `{}` | {} |",
                f.severity,
                f.rule.code,
                f.change.path.to_string().replace('`', "'"),
                change
            );
        }
        out.push('\n');
    }

    let compatible: Vec<&Evaluated> = evaluation
        .findings
        .iter()
        .filter(|e| e.status == Status::Active && e.finding.severity == Severity::Info)
        .collect();
    if !compatible.is_empty() {
        let _ = writeln!(
            out,
            "<details><summary>{} compatible changes</summary>\n",
            compatible.len()
        );
        for e in compatible {
            let _ = writeln!(
                out,
                "- `{}`: {}",
                e.finding.change.path.to_string().replace('`', "'"),
                escape(&e.finding.message)
            );
        }
        out.push_str("\n</details>\n");
    }
    out
}

/// Escapes characters that would break a table cell or inline formatting.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '|' => out.push_str("\\|"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}
