//! JUnit XML, for CI systems that display test results.
//!
//! Each API is a test suite and each finding a test case; failing findings
//! are failures, other active findings pass, and accepted ones are skipped.

use std::fmt::Write as _;

use faultline_policy::Status;

use crate::{Report, short_witness};

pub(crate) fn render(report: &Report<'_>) -> String {
    let evaluation = report.evaluation;
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<testsuites name=\"faultline\" tests=\"{}\" failures=\"{}\" errors=\"{}\">",
        evaluation.findings.len(),
        evaluation.failing(),
        report.diagnostics.error_count()
    );
    let mut names: Vec<&str> = report.apis.iter().map(|a| a.name.as_str()).collect();
    if names.is_empty() {
        names = evaluation
            .findings
            .iter()
            .map(|e| e.finding.api.as_str())
            .collect();
        names.dedup();
    }
    for api in names {
        let findings: Vec<_> = evaluation
            .findings
            .iter()
            .filter(|e| e.finding.api == api)
            .collect();
        let failures = findings.iter().filter(|e| e.failing).count();
        let _ = writeln!(
            out,
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{failures}\">",
            xml(api),
            findings.len()
        );
        for e in findings {
            let f = &e.finding;
            let _ = write!(
                out,
                "    <testcase classname=\"{}.{}\" name=\"{} {}\"",
                xml(api),
                f.rule.code,
                f.rule.code,
                xml(&f.change.path.to_string())
            );
            if e.failing {
                let mut detail = f.message.clone();
                if let Some(w) = f.witness() {
                    let _ = write!(detail, "\nexample: {}", short_witness(w));
                }
                let _ = writeln!(
                    out,
                    ">\n      <failure message=\"{}\" type=\"{}\">{}</failure>\n    </testcase>",
                    xml(&f.message),
                    f.severity,
                    xml(&detail)
                );
            } else if e.status != Status::Active {
                let _ = writeln!(
                    out,
                    ">\n      <skipped message=\"accepted\"/>\n    </testcase>"
                );
            } else {
                out.push_str("/>\n");
            }
        }
        for violation in evaluation
            .version_violations
            .iter()
            .filter(|v| v.api == api)
        {
            let message = crate::version_message(violation);
            let _ = writeln!(
                out,
                "    <testcase classname=\"{0}.version\" name=\"declared version\">\n      <failure message=\"{1}\" type=\"version\">{1}</failure>\n    </testcase>",
                xml(api),
                xml(&message)
            );
        }
        out.push_str("  </testsuite>\n");
    }
    out.push_str("</testsuites>\n");
    out
}

fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\n' | '\t' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}
