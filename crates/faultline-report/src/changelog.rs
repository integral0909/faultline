//! Release notes: what changed, grouped the way changelogs are read.
//!
//! Every finding appears, whatever its status: a waived break is still a
//! break for the people upgrading.

use std::fmt::Write as _;

use faultline_diff::ChangeKind;
use faultline_rules::{Finding, Severity, is_addition};

use crate::Report;

/// The section a finding belongs in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Section {
    Breaking,
    Deprecated,
    Added,
    Changed,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Breaking => "Breaking changes",
            Section::Deprecated => "Deprecated",
            Section::Added => "Added",
            Section::Changed => "Changed",
        }
    }

    fn of(finding: &Finding) -> Section {
        match &finding.change.kind {
            _ if finding.severity == Severity::Breaking => Section::Breaking,
            ChangeKind::DeprecatedChanged { deprecated: true } => Section::Deprecated,
            kind if finding.severity == Severity::Info && is_addition(kind) => Section::Added,
            _ => Section::Changed,
        }
    }
}

pub(crate) fn render(report: &Report<'_>) -> String {
    let evaluation = report.evaluation;
    let mut out = String::from("# Changelog\n");
    let mut names: Vec<&str> = report.apis.iter().map(|a| a.name.as_str()).collect();
    for e in &evaluation.findings {
        if !names.contains(&e.finding.api.as_str()) {
            names.push(&e.finding.api);
        }
    }
    for name in names {
        let summary = report.apis.iter().find(|a| a.name == name);
        let _ = write!(out, "\n## {name}");
        if let Some(api) = summary {
            match (&api.old_version, &api.new_version) {
                (Some(old), Some(new)) if old != new => {
                    let _ = write!(out, " {new} (from {old})");
                }
                (_, Some(new)) => {
                    let _ = write!(out, " {new}");
                }
                _ => {}
            }
            let _ = write!(out, "\n\nSuggested version bump: **{}**.\n", api.bump);
        } else {
            out.push('\n');
        }
        let mut findings: Vec<&Finding> = evaluation
            .findings
            .iter()
            .map(|e| &e.finding)
            .filter(|f| f.api == name)
            .collect();
        if findings.is_empty() {
            out.push_str("\nNo changes.\n");
            continue;
        }
        findings.sort_by_key(|f| Section::of(f));
        let mut current = None;
        for finding in findings {
            let section = Section::of(finding);
            if current != Some(section) {
                let _ = write!(out, "\n### {}\n\n", section.title());
                current = Some(section);
            }
            let _ = writeln!(out, "- `{}`: {}", finding.change.path, finding.message);
        }
    }
    for violation in &evaluation.version_violations {
        let _ = write!(
            out,
            "\n> **Note:** {}.\n",
            crate::version_message(violation)
        );
    }
    out
}
