//! Applying a policy to findings: overrides, waivers, baseline, verdict.

use faultline_rules::{Finding, Severity};

use crate::baseline::Baseline;
use crate::config::{Config, FailOn, Override, Waiver};
use crate::date::Date;

/// Why a finding does not count against the check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Counts according to its severity.
    Active,
    /// Accepted by an unexpired waiver.
    Waived(Waiver),
    /// Present in the baseline.
    Baselined,
}

/// A finding after policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluated {
    pub finding: Finding,
    /// The severity before any override, if an override changed it.
    pub original_severity: Option<Severity>,
    pub status: Status,
    /// Whether this finding fails the check.
    pub failing: bool,
}

/// Process exit codes, stable across releases.
pub mod exit {
    pub const PASSED: i32 = 0;
    pub const FINDINGS: i32 = 1;
    pub const INVALID_INPUT: i32 = 2;
    pub const INTERNAL: i32 = 3;
    pub const EXPIRED_WAIVER: i32 = 4;
}

/// The result of applying a policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub findings: Vec<Evaluated>,
    /// Waivers whose expiry date has passed. They no longer apply, and their
    /// presence fails the check until they are renewed or removed.
    pub expired_waivers: Vec<Waiver>,
    /// Waivers that matched no finding: the problem is gone and the waiver
    /// can be deleted.
    pub unused_waivers: Vec<Waiver>,
    pub fail_on: FailOn,
}

impl Evaluation {
    pub fn failing(&self) -> usize {
        self.findings.iter().filter(|e| e.failing).count()
    }

    /// `0` when passing, `1` for failing findings, `4` for expired waivers.
    pub fn exit_code(&self) -> i32 {
        if self.failing() > 0 {
            exit::FINDINGS
        } else if !self.expired_waivers.is_empty() {
            exit::EXPIRED_WAIVER
        } else {
            exit::PASSED
        }
    }

    pub fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|e| e.status == Status::Active && e.finding.severity == severity)
            .count()
    }
}

/// Applies `config` (and an optional baseline) to `findings` as of `today`.
pub fn evaluate(
    config: &Config,
    baseline: Option<&Baseline>,
    findings: Vec<Finding>,
    today: Date,
) -> Evaluation {
    let (live, expired): (Vec<&Waiver>, Vec<&Waiver>) =
        config.waivers.iter().partition(|w| w.expires >= today);
    let mut used = vec![false; live.len()];

    let findings = findings
        .into_iter()
        .map(|mut finding| {
            let original = finding.severity;
            if let Some(o) = config.overrides.iter().rev().find(|o| matches(o, &finding)) {
                finding.severity = o.severity;
            }
            let status = if let Some(i) = live.iter().position(|w| w.finding == finding.id) {
                used[i] = true;
                Status::Waived(live[i].clone())
            } else if baseline.is_some_and(|b| b.contains(&finding.id)) {
                Status::Baselined
            } else {
                Status::Active
            };
            let failing = status == Status::Active && config.fail_on.fails(finding.severity);
            Evaluated {
                original_severity: (finding.severity != original).then_some(original),
                finding,
                status,
                failing,
            }
        })
        .collect();

    Evaluation {
        findings,
        expired_waivers: expired.into_iter().cloned().collect(),
        unused_waivers: live
            .iter()
            .zip(&used)
            .filter(|&(_, used)| !used)
            .map(|(w, _)| (*w).clone())
            .collect(),
        fail_on: config.fail_on,
    }
}

/// Whether an override applies to a finding. Later overrides win, so the
/// most specific configuration can be written last.
fn matches(o: &Override, finding: &Finding) -> bool {
    if o.rule.is_some_and(|code| code != finding.rule.code) {
        return false;
    }
    if o.api.as_ref().is_some_and(|api| api != &finding.api) {
        return false;
    }
    if o.paths.is_empty() {
        return true;
    }
    let path = finding.change.path.to_string();
    o.paths.iter().any(|glob| glob.matches(&path))
}
