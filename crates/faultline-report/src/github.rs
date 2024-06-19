//! GitHub Actions workflow annotations
//! (`::error file=...,line=...::message`).

use std::fmt::Write as _;

use faultline_policy::Status;
use faultline_rules::Severity;

use crate::location::Location;
use crate::{Report, short_witness};

pub(crate) fn render(report: &Report<'_>) -> String {
    let mut out = String::new();
    for diagnostic in report.diagnostics.iter().filter(|d| d.is_error()) {
        let location = diagnostic
            .primary_span()
            .and_then(|s| Location::resolve(report.sources, s));
        annotation(
            &mut out,
            "error",
            location.as_ref(),
            "input error",
            &diagnostic.message,
        );
    }
    for violation in &report.evaluation.version_violations {
        annotation(
            &mut out,
            "error",
            None,
            "version bump",
            &crate::version_message(violation),
        );
    }
    for e in &report.evaluation.findings {
        if e.status != Status::Active {
            continue;
        }
        let f = &e.finding;
        let level = match f.severity {
            Severity::Breaking => "error",
            Severity::Risky => "warning",
            Severity::Info => "notice",
        };
        let location =
            Location::for_annotation(report.sources, &[f.change.new_span, f.change.old_span]);
        let mut message = format!("{}: {}", f.change.path, f.message);
        if let Some(w) = f.witness() {
            let _ = write!(message, "\nexample: {}", short_witness(w));
        }
        let title = format!("{} {}", f.rule.code, f.rule.name);
        annotation(&mut out, level, location.as_ref(), &title, &message);
    }
    out
}

fn annotation(
    out: &mut String,
    level: &str,
    location: Option<&Location>,
    title: &str,
    message: &str,
) {
    let mut properties = Vec::new();
    if let Some(l) = location {
        properties.push(format!("file={}", property(&l.file)));
        properties.push(format!("line={}", l.line));
        properties.push(format!("col={}", l.column));
        properties.push(format!("endLine={}", l.end_line));
        properties.push(format!("endColumn={}", l.end_column));
    }
    properties.push(format!("title={}", property(title)));
    let _ = writeln!(out, "::{level} {}::{}", properties.join(","), data(message));
}

/// Escaping for the message part of a workflow command.
fn data(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escaping for property values, which additionally reserve `:` and `,`.
fn property(text: &str) -> String {
    data(text).replace(':', "%3A").replace(',', "%2C")
}

#[cfg(test)]
mod tests {
    use super::{data, property};

    #[test]
    fn workflow_command_escaping() {
        assert_eq!(data("50%\nnext"), "50%25%0Anext");
        assert_eq!(property("a:b,c"), "a%3Ab%2Cc");
    }
}
