//! Policy: what a team has decided about findings.
//!
//! * [`Config`] is the validated `faultline.toml`: the APIs to check, when a
//!   check fails, severity overrides, and waivers.
//! * [`Baseline`] records accepted findings when adopting Faultline on an
//!   existing API.
//! * [`evaluate`] applies all of it to a set of findings and produces an
//!   [`Evaluation`] with a verdict and exit code.
//!
//! Waivers must expire. A waiver is a decision to accept a known break *for
//! now*; an expired waiver fails the check until someone renews or removes it,
//! so accepted risk cannot quietly become permanent.

mod baseline;
mod config;
mod date;
mod evaluate;
mod glob;

pub use baseline::{Baseline, BaselineEntry, BaselineError};
pub use config::{
    ApiConfig, ApiFormat, Config, ConfigError, FailOn, MAX_REDIRECTS, MAX_TIMEOUT_SECONDS,
    NetworkConfig, Override, SchemaRole, UsageConfig, Versioning, Waiver,
};
pub use date::Date;
pub use evaluate::{Evaluated, Evaluation, Release, Status, VersionViolation, evaluate, exit};
pub use glob::Glob;
