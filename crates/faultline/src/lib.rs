//! Faultline finds breaking changes in API contracts.
//!
//! This crate is the library entry point. It loads two versions of an API
//! from disk, lowers them into the format-independent IR, compares them, and
//! classifies every change; [`faultline_policy`] then applies a team's
//! configuration and [`faultline_report`] renders the result.
//!
//! ```no_run
//! use faultline::{Engine, Source, Target};
//! use faultline_policy::{Config, Date, evaluate};
//!
//! let engine = Engine::default();
//! let target = Target::new("petstore", Source::new(".", "v1/openapi.yaml"), Source::new(".", "v2/openapi.yaml"));
//! let comparison = engine.compare(&[target]);
//! let evaluation = evaluate(&Config::default(), None, comparison.findings.clone(), Date::today());
//! println!("{} failing findings", evaluation.failing());
//! ```

pub mod codes;
mod engine;

pub use engine::{Comparison, Engine, Lowered, Source, Target, default_api_name, detect_format};
pub use faultline_policy::{ApiFormat, SchemaRole};
