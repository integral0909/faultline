//! A self-contained HTML page, for CI systems that keep reports as build
//! artifacts. It loads nothing from elsewhere and follows the reader's
//! light or dark preference.

use std::fmt::Write as _;

use faultline_policy::{Evaluated, Status};
use faultline_rules::Severity;

use crate::location::Location;
use crate::{Report, TOOL, TOOL_URI, TOOL_VERSION, short_witness};

const STYLE: &str = "
:root { --bg: #ffffff; --fg: #1f2328; --muted: #59636e; --line: #d1d9e0;
  --breaking: #cf222e; --risky: #9a6700; --info: #0969da; --code: #f6f8fa; }
@media (prefers-color-scheme: dark) {
  :root { --bg: #0d1117; --fg: #e6edf3; --muted: #9198a1; --line: #3d444d;
    --breaking: #ff7b72; --risky: #d29922; --info: #58a6ff; --code: #151b23; }
}
body { background: var(--bg); color: var(--fg); margin: 0 auto; max-width: 72rem;
  padding: 1.5rem 1rem; font: 15px/1.5 system-ui, sans-serif; }
h1 { font-size: 1.4rem; } h2 { font-size: 1.1rem; margin-top: 2rem; }
table { border-collapse: collapse; width: 100%; }
th, td { border-bottom: 1px solid var(--line); padding: .4rem .5rem; text-align: left;
  vertical-align: top; }
th { color: var(--muted); font-weight: 600; }
td.n { text-align: right; font-variant-numeric: tabular-nums; }
code { background: var(--code); border-radius: 4px; padding: .1rem .3rem;
  font: 13px/1.4 ui-monospace, monospace; overflow-wrap: anywhere; }
.breaking { color: var(--breaking); font-weight: 600; }
.risky { color: var(--risky); font-weight: 600; }
.info { color: var(--info); }
.muted, footer { color: var(--muted); }
summary { cursor: pointer; margin: 1rem 0 .5rem; }
.table { overflow-x: auto; }
";

pub(crate) fn render(report: &Report<'_>) -> String {
    let evaluation = report.evaluation;
    let mut out =
        String::from("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    let _ = writeln!(
        out,
        "<title>Faultline report</title>\n<style>{STYLE}</style>\n</head>\n<body>"
    );
    let _ = writeln!(out, "<h1>{}</h1>", escape(&crate::headline(report)));
    for violation in &evaluation.version_violations {
        let _ = writeln!(
            out,
            "<p><strong>Version:</strong> {}</p>",
            escape(&crate::version_message(violation))
        );
    }
    summary(&mut out, report);
    errors(&mut out, report);

    let active = |e: &&Evaluated| e.status == Status::Active;
    let serious: Vec<&Evaluated> = evaluation
        .findings
        .iter()
        .filter(active)
        .filter(|e| e.finding.severity > Severity::Info)
        .collect();
    if !serious.is_empty() {
        out.push_str("<h2>Findings</h2>\n");
        findings(&mut out, report, &serious);
    }
    let compatible: Vec<&Evaluated> = evaluation
        .findings
        .iter()
        .filter(active)
        .filter(|e| e.finding.severity == Severity::Info)
        .collect();
    if !compatible.is_empty() {
        let _ = writeln!(
            out,
            "<details><summary>{} compatible changes</summary>",
            compatible.len()
        );
        findings(&mut out, report, &compatible);
        out.push_str("</details>\n");
    }
    let accepted: Vec<&Evaluated> = evaluation
        .findings
        .iter()
        .filter(|e| e.status != Status::Active)
        .collect();
    if !accepted.is_empty() {
        let _ = writeln!(
            out,
            "<details><summary>{} waived or baselined</summary>",
            accepted.len()
        );
        findings(&mut out, report, &accepted);
        out.push_str("</details>\n");
    }
    let _ = writeln!(
        out,
        "<footer><p>{TOOL} {TOOL_VERSION} · <a href=\"{TOOL_URI}\">{TOOL_URI}</a></p></footer>\n</body>\n</html>"
    );
    out
}

fn summary(out: &mut String, report: &Report<'_>) {
    if report.apis.is_empty() {
        return;
    }
    out.push_str("<div class=\"table\"><table>\n<tr><th>API</th><th>Versions</th><th>Breaking</th><th>Risky</th><th>Compatible</th><th>Suggested bump</th></tr>\n");
    for api in report.apis {
        let count = |severity| {
            report
                .evaluation
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
            (Some(old), Some(new)) => format!("{} → {}", escape(old), escape(new)),
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "<tr><td>{}</td><td>{versions}</td><td class=\"n breaking\">{}</td><td class=\"n risky\">{}</td><td class=\"n\">{}</td><td>{}</td></tr>",
            escape(&api.name),
            count(Severity::Breaking),
            count(Severity::Risky),
            count(Severity::Info),
            api.bump
        );
    }
    out.push_str("</table></div>\n");
}

fn errors(out: &mut String, report: &Report<'_>) {
    let errors: Vec<_> = report.diagnostics.iter().filter(|d| d.is_error()).collect();
    if errors.is_empty() {
        return;
    }
    out.push_str("<h2>Input errors</h2>\n<ul>\n");
    for diagnostic in errors {
        let at = diagnostic
            .primary_span()
            .and_then(|s| Location::resolve(report.sources, s))
            .map(|l| format!(" <span class=\"muted\">{}</span>", escape(&place(&l))))
            .unwrap_or_default();
        let code = diagnostic
            .code
            .map(|c| format!("<code>{c}</code> "))
            .unwrap_or_default();
        let _ = writeln!(out, "<li>{code}{}{at}</li>", escape(&diagnostic.message));
    }
    out.push_str("</ul>\n");
}

fn findings(out: &mut String, report: &Report<'_>, list: &[&Evaluated]) {
    out.push_str("<div class=\"table\"><table>\n<tr><th>Severity</th><th>Rule</th><th>Where</th><th>Change</th><th>Location</th></tr>\n");
    for e in list {
        let f = &e.finding;
        let class = match f.severity {
            Severity::Breaking => "breaking",
            Severity::Risky => "risky",
            Severity::Info => "info",
        };
        let mut change = escape(&f.message);
        if let Some(w) = f.witness() {
            let _ = write!(
                change,
                "<br><span class=\"muted\">example:</span> <code>{}</code>",
                escape(&short_witness(w))
            );
        }
        match &e.status {
            Status::Waived(w) => {
                let _ = write!(
                    change,
                    "<br><span class=\"muted\">waived until {}: {}</span>",
                    w.expires,
                    escape(&w.reason)
                );
            }
            Status::Baselined => change.push_str("<br><span class=\"muted\">baselined</span>"),
            Status::Active => {}
        }
        let location =
            Location::for_annotation(report.sources, &[f.change.new_span, f.change.old_span])
                .map(|l| escape(&place(&l)))
                .unwrap_or_default();
        let _ = writeln!(
            out,
            "<tr id=\"{id}\"><td class=\"{class}\">{severity}</td><td><a href=\"{TOOL_URI}/blob/main/docs/rules.md#{anchor}\" title=\"{name}\">{code}</a></td><td><code>{path}</code></td><td>{change}</td><td class=\"muted\">{location}</td></tr>",
            id = escape(&f.id),
            severity = f.severity,
            anchor = f.rule.code.to_ascii_lowercase(),
            name = escape(f.rule.name),
            code = f.rule.code,
            path = escape(&f.change.path.to_string()),
        );
    }
    out.push_str("</table></div>\n");
}

/// `file:line`, with the revision it was read from.
fn place(location: &Location) -> String {
    match &location.revision {
        Some(rev) => format!("{rev}:{}:{}", location.file, location.line),
        None => format!("{}:{}", location.file, location.line),
    }
}

/// Escapes text for HTML content and attribute values.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::escape;

    #[test]
    fn escaping() {
        assert_eq!(
            escape("<script>alert(\"x\" & 'y')</script>"),
            "&lt;script&gt;alert(&quot;x&quot; &amp; &#39;y&#39;)&lt;/script&gt;"
        );
    }
}
