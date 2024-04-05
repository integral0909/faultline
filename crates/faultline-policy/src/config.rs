//! `faultline.toml`: the workspace configuration.
//!
//! Parsing is strict: unknown keys are errors, because a misspelled key in a
//! CI gate (`fail-on` for `fail_on`) would otherwise silently weaken it.

use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

use faultline_rules::Severity;

use crate::date::Date;
use crate::glob::Glob;

/// The raw file, as written.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    workspace: RawWorkspace,
    #[serde(default)]
    api: Vec<RawApi>,
    #[serde(default)]
    policy: RawPolicy,
    #[serde(default)]
    waiver: Vec<RawWaiver>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkspace {
    root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawApi {
    name: String,
    entry: PathBuf,
    format: Option<String>,
    role: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    fail_on: Option<String>,
    baseline: Option<PathBuf>,
    #[serde(default, rename = "override")]
    overrides: Vec<RawOverride>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOverride {
    rule: String,
    #[serde(default)]
    paths: Vec<String>,
    api: Option<String>,
    severity: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWaiver {
    finding: String,
    reason: String,
    owner: Option<String>,
    expires: String,
}

/// The format of an API's description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiFormat {
    OpenApi,
    JsonSchema,
}

impl ApiFormat {
    pub fn parse(text: &str) -> Option<ApiFormat> {
        match text {
            "openapi" => Some(ApiFormat::OpenApi),
            "jsonschema" | "json-schema" => Some(ApiFormat::JsonSchema),
            _ => None,
        }
    }
}

/// Which direction a standalone schema's documents flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaRole {
    Input,
    Output,
    Both,
}

impl SchemaRole {
    pub fn parse(text: &str) -> Option<SchemaRole> {
        match text {
            "input" => Some(SchemaRole::Input),
            "output" => Some(SchemaRole::Output),
            "both" => Some(SchemaRole::Both),
            _ => None,
        }
    }
}

/// One API under check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiConfig {
    pub name: String,
    /// Relative to the workspace root.
    pub entry: PathBuf,
    /// Detected from the document when absent.
    pub format: Option<ApiFormat>,
    pub role: SchemaRole,
}

/// When a check fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailOn {
    Breaking,
    Risky,
    Never,
}

impl FailOn {
    pub fn parse(text: &str) -> Option<FailOn> {
        match text {
            "breaking" => Some(FailOn::Breaking),
            "risky" => Some(FailOn::Risky),
            "never" => Some(FailOn::Never),
            _ => None,
        }
    }

    /// Whether a finding of `severity` fails the check.
    pub fn fails(self, severity: Severity) -> bool {
        match self {
            FailOn::Breaking => severity >= Severity::Breaking,
            FailOn::Risky => severity >= Severity::Risky,
            FailOn::Never => false,
        }
    }
}

/// A severity override for matching findings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Override {
    /// Rule code, or `None` for every rule (`rule = "*"`).
    pub rule: Option<&'static str>,
    pub paths: Vec<Glob>,
    pub api: Option<String>,
    pub severity: Severity,
}

/// A temporary acceptance of one finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiver {
    pub finding: String,
    pub reason: String,
    pub owner: Option<String>,
    pub expires: Date,
}

/// The validated configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub root: Option<PathBuf>,
    pub apis: Vec<ApiConfig>,
    pub fail_on: FailOn,
    pub baseline: Option<PathBuf>,
    pub overrides: Vec<Override>,
    pub waivers: Vec<Waiver>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            root: None,
            apis: Vec::new(),
            fail_on: FailOn::Breaking,
            baseline: None,
            overrides: Vec::new(),
            waivers: Vec::new(),
        }
    }
}

/// Everything wrong with a configuration file, reported together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub problems: Vec<String>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid configuration: {}", self.problems.join("; "))
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Parses and validates the text of a `faultline.toml`.
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError {
            problems: vec![e.to_string().trim().to_owned()],
        })?;
        let mut problems = Vec::new();
        let apis = apis(raw.api, &mut problems);
        let fail_on = match raw.policy.fail_on.as_deref() {
            None => FailOn::Breaking,
            Some(text) => FailOn::parse(text).unwrap_or_else(|| {
                problems.push(format!(
                    "[policy] fail_on must be \"breaking\", \"risky\" or \"never\", not \"{text}\""
                ));
                FailOn::Breaking
            }),
        };
        let overrides = overrides(raw.policy.overrides, &apis, &mut problems);
        let waivers = waivers(raw.waiver, &mut problems);
        if !problems.is_empty() {
            return Err(ConfigError { problems });
        }
        Ok(Config {
            root: raw.workspace.root,
            apis,
            fail_on,
            baseline: raw.policy.baseline,
            overrides,
            waivers,
        })
    }
}

fn apis(raw: Vec<RawApi>, problems: &mut Vec<String>) -> Vec<ApiConfig> {
    let mut apis: Vec<ApiConfig> = Vec::new();
    for (i, api) in raw.into_iter().enumerate() {
        let at = format!("[[api]] #{} ({})", i + 1, api.name);
        if api.name.trim().is_empty() {
            problems.push(format!("{at}: name must not be empty"));
        }
        if apis.iter().any(|a| a.name == api.name) {
            problems.push(format!("{at}: another api has the same name"));
        }
        let format = api.format.as_deref().and_then(|text| {
            ApiFormat::parse(text).or_else(|| {
                problems.push(format!(
                    "{at}: format must be \"openapi\" or \"jsonschema\", not \"{text}\""
                ));
                None
            })
        });
        let role = api.role.as_deref().map_or(SchemaRole::Both, |text| {
            SchemaRole::parse(text).unwrap_or_else(|| {
                problems.push(format!(
                    "{at}: role must be \"input\", \"output\" or \"both\", not \"{text}\""
                ));
                SchemaRole::Both
            })
        });
        apis.push(ApiConfig {
            name: api.name,
            entry: api.entry,
            format,
            role,
        });
    }
    apis
}

fn overrides(
    raw: Vec<RawOverride>,
    apis: &[ApiConfig],
    problems: &mut Vec<String>,
) -> Vec<Override> {
    let mut out = Vec::new();
    for (i, raw) in raw.into_iter().enumerate() {
        let at = format!("[[policy.override]] #{}", i + 1);
        let rule = if raw.rule == "*" {
            None
        } else if let Some(rule) = faultline_rules::catalog::find(&raw.rule) {
            Some(rule.code)
        } else {
            problems.push(format!("{at}: unknown rule \"{}\"", raw.rule));
            continue;
        };
        let Some(severity) = Severity::parse(&raw.severity) else {
            problems.push(format!(
                "{at}: severity must be \"breaking\", \"risky\" or \"info\", not \"{}\"",
                raw.severity
            ));
            continue;
        };
        if let Some(api) = &raw.api
            && !apis.iter().any(|a| &a.name == api)
        {
            problems.push(format!("{at}: no api named \"{api}\""));
        }
        out.push(Override {
            rule,
            paths: raw.paths.iter().map(|p| Glob::new(p)).collect(),
            api: raw.api,
            severity,
        });
    }
    out
}

fn waivers(raw: Vec<RawWaiver>, problems: &mut Vec<String>) -> Vec<Waiver> {
    let mut out = Vec::new();
    for (i, raw) in raw.into_iter().enumerate() {
        let at = format!("[[waiver]] #{} ({})", i + 1, raw.finding);
        let well_formed =
            raw.finding.len() == 12 && raw.finding.bytes().all(|b| b.is_ascii_hexdigit());
        if !well_formed {
            problems.push(format!("{at}: finding must be a 12-digit finding id"));
        }
        if raw.reason.trim().is_empty() {
            problems.push(format!("{at}: a waiver needs a reason"));
        }
        let Some(expires) = Date::parse(&raw.expires) else {
            problems.push(format!("{at}: expires must be a date like 2026-12-31"));
            continue;
        };
        out.push(Waiver {
            finding: raw.finding.to_ascii_lowercase(),
            reason: raw.reason,
            owner: raw.owner,
            expires,
        });
    }
    out
}
