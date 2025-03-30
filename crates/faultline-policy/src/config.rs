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
    #[serde(default)]
    network: RawNetwork,
    #[serde(default)]
    usage: RawUsage,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUsage {
    index: Option<PathBuf>,
    unused_severity: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNetwork {
    #[serde(default)]
    allow_remote: bool,
    #[serde(default)]
    allowed_hosts: Vec<String>,
    max_bytes: Option<u64>,
    timeout_seconds: Option<u64>,
    max_redirects: Option<u32>,
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
    #[serde(default)]
    overlays: Vec<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    fail_on: Option<String>,
    versioning: Option<String>,
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
    Protobuf,
    GraphQl,
    Avro,
    AsyncApi,
    /// A Protobuf file's services as the HTTP/JSON API their
    /// `google.api.http` rules expose.
    ProtobufHttp,
    /// XML Schema: the global elements, used in the schema's role.
    Xsd,
    /// WSDL 1.1 or 2.0 services.
    Wsdl,
    /// RAML 1.0 or 0.8 APIs.
    Raml,
    /// Apache Thrift services and types.
    Thrift,
    /// JSON Type Definition schemas.
    Jtd,
    /// Smithy models.
    Smithy,
}

impl ApiFormat {
    pub const ALL: [ApiFormat; 13] = [
        ApiFormat::OpenApi,
        ApiFormat::AsyncApi,
        ApiFormat::JsonSchema,
        ApiFormat::Protobuf,
        ApiFormat::ProtobufHttp,
        ApiFormat::GraphQl,
        ApiFormat::Avro,
        ApiFormat::Xsd,
        ApiFormat::Wsdl,
        ApiFormat::Raml,
        ApiFormat::Thrift,
        ApiFormat::Jtd,
        ApiFormat::Smithy,
    ];

    /// The name `parse` reads back.
    pub fn as_str(self) -> &'static str {
        match self {
            ApiFormat::OpenApi => "openapi",
            ApiFormat::JsonSchema => "jsonschema",
            ApiFormat::Protobuf => "protobuf",
            ApiFormat::GraphQl => "graphql",
            ApiFormat::Avro => "avro",
            ApiFormat::AsyncApi => "asyncapi",
            ApiFormat::ProtobufHttp => "protobuf-http",
            ApiFormat::Xsd => "xsd",
            ApiFormat::Wsdl => "wsdl",
            ApiFormat::Raml => "raml",
            ApiFormat::Thrift => "thrift",
            ApiFormat::Jtd => "jtd",
            ApiFormat::Smithy => "smithy",
        }
    }

    pub fn parse(text: &str) -> Option<ApiFormat> {
        match text {
            "openapi" => Some(ApiFormat::OpenApi),
            "jsonschema" | "json-schema" => Some(ApiFormat::JsonSchema),
            "protobuf" | "proto" => Some(ApiFormat::Protobuf),
            "graphql" => Some(ApiFormat::GraphQl),
            "avro" => Some(ApiFormat::Avro),
            "asyncapi" => Some(ApiFormat::AsyncApi),
            "protobuf-http" => Some(ApiFormat::ProtobufHttp),
            "xsd" | "xml-schema" => Some(ApiFormat::Xsd),
            "wsdl" => Some(ApiFormat::Wsdl),
            "raml" => Some(ApiFormat::Raml),
            "thrift" => Some(ApiFormat::Thrift),
            "jtd" | "json-type-definition" => Some(ApiFormat::Jtd),
            "smithy" => Some(ApiFormat::Smithy),
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
    pub fn as_str(self) -> &'static str {
        match self {
            SchemaRole::Input => "input",
            SchemaRole::Output => "output",
            SchemaRole::Both => "both",
        }
    }

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
    /// Overlays applied to the entry document of both versions, in order;
    /// relative to the workspace root.
    pub overlays: Vec<PathBuf>,
}

/// How declared API versions are checked against the changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Versioning {
    /// Declared versions are reported, not checked.
    #[default]
    Off,
    /// A declared version must increase by at least the bump the changes
    /// need, by Semantic Versioning (`MAJOR.MINOR.PATCH`).
    Semver,
}

/// When a check fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailOn {
    Breaking,
    Risky,
    Never,
}

impl FailOn {
    pub fn as_str(self) -> &'static str {
        match self {
            FailOn::Breaking => "breaking",
            FailOn::Risky => "risky",
            FailOn::Never => "never",
        }
    }

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

/// `[network]`: whether and where remote references may be fetched.
///
/// Host patterns are checked when the network policy is built from this,
/// by the crate that enforces it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkConfig {
    pub allow_remote: bool,
    pub allowed_hosts: Vec<String>,
    pub max_bytes: Option<u64>,
    pub timeout_seconds: Option<u64>,
    pub max_redirects: Option<u32>,
}

/// `[usage]`: an index of what clients use, and how changes to unused
/// parts are treated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageConfig {
    /// The usage index file, relative to the workspace root.
    pub index: Option<PathBuf>,
    /// The most severe a finding about an unused part of the API can be.
    pub unused_severity: Severity,
}

impl Default for UsageConfig {
    fn default() -> Self {
        Self {
            index: None,
            unused_severity: Severity::Risky,
        }
    }
}

/// Longest request timeout accepted in `[network]`.
pub const MAX_TIMEOUT_SECONDS: u64 = 300;
/// Most redirects accepted in `[network]`.
pub const MAX_REDIRECTS: u32 = 20;

/// The validated configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub root: Option<PathBuf>,
    pub apis: Vec<ApiConfig>,
    pub fail_on: FailOn,
    pub versioning: Versioning,
    pub baseline: Option<PathBuf>,
    pub overrides: Vec<Override>,
    pub waivers: Vec<Waiver>,
    pub network: NetworkConfig,
    pub usage: UsageConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            root: None,
            apis: Vec::new(),
            fail_on: FailOn::Breaking,
            versioning: Versioning::Off,
            baseline: None,
            overrides: Vec::new(),
            waivers: Vec::new(),
            network: NetworkConfig::default(),
            usage: UsageConfig::default(),
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
        let versioning = match raw.policy.versioning.as_deref() {
            None | Some("off") => Versioning::Off,
            Some("semver") => Versioning::Semver,
            Some(text) => {
                problems.push(format!(
                    "[policy] versioning must be \"off\" or \"semver\", not \"{text}\""
                ));
                Versioning::Off
            }
        };
        let overrides = overrides(raw.policy.overrides, &apis, &mut problems);
        let waivers = waivers(raw.waiver, &mut problems);
        let network = network(raw.network, &mut problems);
        let usage = UsageConfig {
            index: raw.usage.index,
            unused_severity: match raw.usage.unused_severity.as_deref() {
                None => Severity::Risky,
                Some(text) => Severity::parse(text).unwrap_or_else(|| {
                    problems.push(format!(
                        "[usage] unused_severity must be \"breaking\", \"risky\" or \"info\", not \"{text}\""
                    ));
                    Severity::Risky
                }),
            },
        };
        if !problems.is_empty() {
            return Err(ConfigError { problems });
        }
        Ok(Config {
            root: raw.workspace.root,
            apis,
            fail_on,
            versioning,
            baseline: raw.policy.baseline,
            overrides,
            waivers,
            network,
            usage,
        })
    }
}

fn network(raw: RawNetwork, problems: &mut Vec<String>) -> NetworkConfig {
    if raw.allow_remote && raw.allowed_hosts.is_empty() {
        problems
            .push("[network] allow_remote needs at least one entry in allowed_hosts".to_owned());
    }
    if raw.max_bytes == Some(0) {
        problems.push("[network] max_bytes must be positive".to_owned());
    }
    if let Some(t) = raw.timeout_seconds
        && !(1..=MAX_TIMEOUT_SECONDS).contains(&t)
    {
        problems.push(format!(
            "[network] timeout_seconds must be between 1 and {MAX_TIMEOUT_SECONDS}"
        ));
    }
    if raw.max_redirects.is_some_and(|r| r > MAX_REDIRECTS) {
        problems.push(format!(
            "[network] max_redirects must be at most {MAX_REDIRECTS}"
        ));
    }
    NetworkConfig {
        allow_remote: raw.allow_remote,
        allowed_hosts: raw.allowed_hosts,
        max_bytes: raw.max_bytes,
        timeout_seconds: raw.timeout_seconds,
        max_redirects: raw.max_redirects,
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
                let names: Vec<String> = ApiFormat::ALL
                    .iter()
                    .map(|f| format!("\"{}\"", f.as_str()))
                    .collect();
                problems.push(format!(
                    "{at}: format must be one of {}, not \"{text}\"",
                    names.join(", ")
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
            overlays: api.overlays,
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
            let hint = faultline_rules::catalog::suggest(&raw.rule)
                .map(|rule| format!(" (did you mean \"{}\"?)", rule.name))
                .unwrap_or_default();
            problems.push(format!("{at}: unknown rule \"{}\"{hint}", raw.rule));
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
