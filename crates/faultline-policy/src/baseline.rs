//! Baselines: a recorded set of accepted findings.
//!
//! Adopting Faultline on an API that already has problems should not require
//! fixing them all first. A baseline records the findings present when it was
//! written; those findings stop failing checks, while any new finding still
//! does. Unlike waivers, baseline entries do not expire — they are removed by
//! rewriting the baseline.

use std::fmt;

use serde::{Deserialize, Serialize};

use faultline_rules::Finding;

const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineEntry {
    pub id: String,
    /// The rest is for humans reading the file; only `id` is matched.
    pub api: String,
    pub rule: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    version: u32,
    pub findings: Vec<BaselineEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaselineError(pub String);

impl fmt::Display for BaselineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid baseline: {}", self.0)
    }
}

impl std::error::Error for BaselineError {}

impl Baseline {
    /// A baseline accepting exactly `findings`, sorted for stable diffs of the
    /// file itself.
    pub fn from_findings<'a>(findings: impl IntoIterator<Item = &'a Finding>) -> Baseline {
        let mut entries: Vec<BaselineEntry> = findings
            .into_iter()
            .map(|f| BaselineEntry {
                id: f.id.clone(),
                api: f.api.clone(),
                rule: f.rule.code.to_owned(),
                path: f.change.path.to_string(),
            })
            .collect();
        entries.sort_by(|a, b| {
            (&a.api, &a.path, &a.rule, &a.id).cmp(&(&b.api, &b.path, &b.rule, &b.id))
        });
        entries.dedup_by(|a, b| a.id == b.id);
        Baseline {
            version: VERSION,
            findings: entries,
        }
    }

    pub fn parse(text: &str) -> Result<Baseline, BaselineError> {
        let baseline: Baseline =
            serde_json::from_str(text).map_err(|e| BaselineError(e.to_string()))?;
        if baseline.version != VERSION {
            return Err(BaselineError(format!(
                "unsupported version {} (expected {VERSION})",
                baseline.version
            )));
        }
        Ok(baseline)
    }

    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("baselines always serialize");
        text.push('\n');
        text
    }

    pub fn contains(&self, id: &str) -> bool {
        self.findings.iter().any(|e| e.id == id)
    }
}
