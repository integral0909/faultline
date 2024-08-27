//! Usage indexes: which operations, parameters and fields clients use.
//!
//! A breaking change to something no client uses is a different risk from
//! one to a hot path. An index built from access logs ([`access_log`]),
//! from the GraphQL documents clients send ([`graphql`]) or from recorded
//! traffic ([`har`]) lets the policy
//! soften findings about unused parts of an API; recorded traffic ([`har`])
//! also says which request fields clients send. Indexes hold counts of
//! templated operations, parameter names and field paths only; request
//! values, bodies, headers and client identities are never read into them.

pub mod access_log;
pub mod graphql;
pub mod har;
mod index;

pub use index::{INDEX_VERSION, Observed, OperationUsage, Source, UsageIndex, field_chain};

use faultline_policy::Evaluation;
use faultline_rules::Severity;

/// Caps the severity of findings about parts of the API that `index` shows
/// unused at `cap`, marks them, and recomputes which findings fail.
pub fn apply(evaluation: &mut Evaluation, index: &UsageIndex, cap: Severity) {
    let fail_on = evaluation.fail_on;
    for evaluated in &mut evaluation.findings {
        if index.observe(&evaluated.finding.change.path) != Observed::Unused {
            continue;
        }
        evaluated.unused = true;
        let finding = &mut evaluated.finding;
        if finding.severity > cap {
            evaluated.original_severity.get_or_insert(finding.severity);
            finding.severity = cap;
        }
        evaluated.failing =
            evaluated.status == faultline_policy::Status::Active && fail_on.fails(finding.severity);
    }
}
