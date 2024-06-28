//! Human-readable output with source excerpts.

use std::fmt::Write as _;

use faultline_policy::{Evaluated, FailOn, Status};
use faultline_rules::Severity;
use faultline_source::{Diagnostic, DiagnosticCode, render::render as render_diagnostic};

use crate::{Report, short_witness};

pub(crate) fn render(report: &Report<'_>) -> String {
    let mut out = String::new();
    for diagnostic in report.diagnostics {
        out.push_str(&render_diagnostic(diagnostic, report.sources));
        out.push('\n');
    }
    findings(report, &mut out);
    waivers(report, &mut out);
    summary(report, &mut out);
    out
}

/// Breaking and risky findings with excerpts, then compatible and accepted
/// ones as single lines.
fn findings(report: &Report<'_>, out: &mut String) {
    let evaluation = report.evaluation;
    let mut serious: Vec<&Evaluated> = evaluation
        .findings
        .iter()
        .filter(|e| e.status == Status::Active && e.finding.severity > Severity::Info)
        .collect();
    // Most severe first; the diff's path order within a severity.
    serious.sort_by_key(|e| std::cmp::Reverse(e.finding.severity));
    for evaluated in serious {
        out.push_str(&finding(report, evaluated));
        out.push('\n');
    }

    let compatible: Vec<&Evaluated> = evaluation
        .findings
        .iter()
        .filter(|e| e.status == Status::Active && e.finding.severity == Severity::Info)
        .collect();
    if !compatible.is_empty() {
        out.push_str("compatible changes:\n");
        for e in compatible {
            let _ = writeln!(
                out,
                "  {} {}: {}",
                e.finding.rule.code, e.finding.change.path, e.finding.message
            );
        }
        out.push('\n');
    }

    let accepted: Vec<&Evaluated> = evaluation
        .findings
        .iter()
        .filter(|e| e.status != Status::Active)
        .collect();
    if !accepted.is_empty() {
        out.push_str("accepted changes:\n");
        for e in accepted {
            let why = match &e.status {
                Status::Waived(w) => format!("waived until {}: {}", w.expires, w.reason),
                Status::Baselined => "in baseline".to_owned(),
                Status::Active => unreachable!("filtered above"),
            };
            let _ = writeln!(
                out,
                "  {}[{}] {}: {} ({why})",
                e.finding.severity, e.finding.rule.code, e.finding.change.path, e.finding.message
            );
        }
        out.push('\n');
    }
}

fn waivers(report: &Report<'_>, out: &mut String) {
    for violation in &report.evaluation.version_violations {
        let _ = writeln!(out, "error: {}", crate::version_message(violation));
    }
    for waiver in &report.evaluation.expired_waivers {
        let _ = writeln!(
            out,
            "error: waiver for {} expired on {} ({})",
            waiver.finding, waiver.expires, waiver.reason
        );
    }
    for waiver in &report.evaluation.unused_waivers {
        let _ = writeln!(
            out,
            "warning: waiver for {} matches no finding and can be removed",
            waiver.finding
        );
    }
}

/// One line per API, then the verdict.
fn summary(report: &Report<'_>, out: &mut String) {
    let evaluation = report.evaluation;
    for api in report.apis {
        let counts = |severity| {
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
        let versions = match (&api.old_version, &api.new_version) {
            (Some(a), Some(b)) if a != b => format!(" {a} -> {b}"),
            (_, Some(b)) => format!(" {b}"),
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "{}{versions}: {} breaking, {} risky, {} compatible; suggested version bump: {}",
            api.name,
            counts(Severity::Breaking),
            counts(Severity::Risky),
            counts(Severity::Info),
            api.bump
        );
    }

    let failing = evaluation.failing();
    let threshold = match evaluation.fail_on {
        FailOn::Breaking => "breaking",
        FailOn::Risky => "risky",
        FailOn::Never => "never",
    };
    let verdict = if report.diagnostics.has_errors() {
        "failed (the inputs have errors)".to_owned()
    } else if failing > 0 {
        let noun = if failing == 1 { "finding" } else { "findings" };
        format!("failed ({failing} {noun} at or above {threshold})")
    } else if !evaluation.version_violations.is_empty() {
        "failed (the declared version does not match the changes)".to_owned()
    } else if !evaluation.expired_waivers.is_empty() {
        "failed (expired waivers)".to_owned()
    } else {
        "passed".to_owned()
    };
    let _ = writeln!(out, "result: {verdict}");
}

fn finding(report: &Report<'_>, evaluated: &Evaluated) -> String {
    let f = &evaluated.finding;
    let change = &f.change;
    let severity = match f.severity {
        Severity::Breaking => faultline_source::Severity::Error,
        Severity::Risky => faultline_source::Severity::Warning,
        Severity::Info => faultline_source::Severity::Note,
    };
    let mut diagnostic = Diagnostic::new(severity, format!("{}: {}", change.path, f.message))
        .with_code(DiagnosticCode::new(f.rule.code));
    match (change.new_span, change.old_span) {
        (Some(new), Some(old)) => {
            diagnostic = diagnostic
                .with_primary(new, "now")
                .with_secondary(old, "before");
        }
        (Some(new), None) => diagnostic = diagnostic.with_primary(new, "now"),
        (None, Some(old)) => diagnostic = diagnostic.with_primary(old, "before"),
        (None, None) => {}
    }
    if let Some(witness) = f.witness() {
        let effect = if change.polarity.is_in() {
            "was accepted and is now rejected"
        } else {
            "may now be returned, and could not be before"
        };
        diagnostic = diagnostic.with_note(format!("example: {} {effect}", short_witness(witness)));
    }
    if evaluated.unused {
        diagnostic = diagnostic.with_note("the usage index shows no client using this");
    }
    if let Some(original) = evaluated.original_severity {
        diagnostic = diagnostic.with_note(format!(
            "severity overridden from {original} by configuration"
        ));
    }
    diagnostic = diagnostic
        .with_note(format!("finding id: {}", f.id))
        .with_help(format!(
            "`faultline explain {}` describes this rule",
            f.rule.code
        ));

    // The renderer names diagnostics by their level ("error", "warning");
    // findings are named by severity instead.
    let rendered = render_diagnostic(&diagnostic, report.sources);
    let level = severity.as_str();
    format!("{}{}", f.severity, &rendered[level.len()..])
}
