//! Reports of a Faultline run in every supported output format.
//!
//! A [`Report`] bundles everything a run produced — input diagnostics, the
//! evaluated findings, per-API summaries — together with the source map that
//! resolves their spans. [`render`] turns it into text for people or into one
//! of the machine formats CI systems consume.

mod github;
mod json;
mod junit;
mod location;
mod markdown;
mod sarif;
mod text;

use std::fmt;

use faultline_policy::Evaluation;
use faultline_rules::Bump;
use faultline_source::{Diagnostics, SourceMap};

pub use location::Location;

/// The name and version reported as the producing tool.
pub const TOOL: &str = "faultline";
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const TOOL_URI: &str = "https://github.com/integral0909/faultline";

/// Version of the JSON report format. Incremented only for incompatible
/// changes; fields may be added without an increment.
pub const JSON_SCHEMA_VERSION: u32 = 1;

/// The outcome for one compared API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiSummary {
    pub name: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub bump: Bump,
}

/// Everything a run produced.
#[derive(Debug)]
pub struct Report<'a> {
    pub sources: &'a SourceMap,
    /// Problems with the inputs (parse errors, unresolved references).
    pub diagnostics: &'a Diagnostics,
    pub apis: &'a [ApiSummary],
    pub evaluation: &'a Evaluation,
}

/// Output formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
    Sarif,
    Markdown,
    Github,
    Junit,
}

impl OutputFormat {
    pub const ALL: [OutputFormat; 6] = [
        OutputFormat::Text,
        OutputFormat::Json,
        OutputFormat::Sarif,
        OutputFormat::Markdown,
        OutputFormat::Github,
        OutputFormat::Junit,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            OutputFormat::Text => "text",
            OutputFormat::Json => "json",
            OutputFormat::Sarif => "sarif",
            OutputFormat::Markdown => "markdown",
            OutputFormat::Github => "github",
            OutputFormat::Junit => "junit",
        }
    }

    pub fn parse(text: &str) -> Option<OutputFormat> {
        Self::ALL.into_iter().find(|f| f.as_str() == text)
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Renders `report` in `format`.
pub fn render(report: &Report<'_>, format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => text::render(report),
        OutputFormat::Json => json::render(report),
        OutputFormat::Sarif => sarif::render(report),
        OutputFormat::Markdown => markdown::render(report),
        OutputFormat::Github => github::render(report),
        OutputFormat::Junit => junit::render(report),
    }
}

/// Shortens a witness for display, keeping it valid enough to read.
pub(crate) fn short_witness(value: &faultline_ir::Literal) -> String {
    const MAX: usize = 120;
    let text = value.to_string();
    if text.chars().count() <= MAX {
        return text;
    }
    let truncated: String = text.chars().take(MAX).collect();
    format!("{truncated}…")
}
