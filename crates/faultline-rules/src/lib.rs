//! The rule catalog and the classification of changes into findings.
//!
//! [`classify`] turns a [`Change`](faultline_diff::Change) into a [`Finding`]
//! with a rule from the [`catalog`], a [`Severity`] and a message.
//! [`suggest_bump`] derives a semantic-version suggestion from a set of
//! findings.

pub mod catalog;
mod classify;
mod semver;

pub use catalog::{Dimension, RULES, Rule};
pub use classify::{Finding, Severity, classify, finding_id};
pub use semver::{Bump, suggest_bump};
