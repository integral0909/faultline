use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use faultline::{Engine, Revision, Source, Target, default_api_name};
use faultline_policy::{
    ApiFormat, Baseline, Config, Date, FailOn, Release, SchemaRole, evaluate, exit,
};
use faultline_report::{Report, render};
use faultline_source::SourceMap;

use crate::{Command, IrCommand, Output, UsageCommand};

/// `--format` values.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Format {
    Openapi,
    Jsonschema,
    Protobuf,
    Graphql,
    Avro,
    Asyncapi,
    /// A `.proto` file's services as the HTTP/JSON API their
    /// `google.api.http` rules expose.
    ProtobufHttp,
    /// XML Schema.
    Xsd,
    /// WSDL 1.1 or 2.0.
    Wsdl,
    /// RAML 1.0 or 0.8.
    Raml,
    /// Apache Thrift IDL.
    Thrift,
    /// JSON Type Definition (RFC 8927).
    Jtd,
    /// Smithy IDL 2.0.
    Smithy,
}

impl From<Format> for ApiFormat {
    fn from(format: Format) -> Self {
        match format {
            Format::Openapi => ApiFormat::OpenApi,
            Format::Jsonschema => ApiFormat::JsonSchema,
            Format::Protobuf => ApiFormat::Protobuf,
            Format::Graphql => ApiFormat::GraphQl,
            Format::Avro => ApiFormat::Avro,
            Format::Asyncapi => ApiFormat::AsyncApi,
            Format::ProtobufHttp => ApiFormat::ProtobufHttp,
            Format::Xsd => ApiFormat::Xsd,
            Format::Wsdl => ApiFormat::Wsdl,
            Format::Raml => ApiFormat::Raml,
            Format::Thrift => ApiFormat::Thrift,
            Format::Jtd => ApiFormat::Jtd,
            Format::Smithy => ApiFormat::Smithy,
        }
    }
}

/// `--role` values.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Role {
    Input,
    Output,
    Both,
}

impl From<Role> for SchemaRole {
    fn from(role: Role) -> Self {
        match role {
            Role::Input => SchemaRole::Input,
            Role::Output => SchemaRole::Output,
            Role::Both => SchemaRole::Both,
        }
    }
}

/// `--fail-on` values.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Threshold {
    Breaking,
    Risky,
    Never,
}

impl From<Threshold> for FailOn {
    fn from(t: Threshold) -> Self {
        match t {
            Threshold::Breaking => FailOn::Breaking,
            Threshold::Risky => FailOn::Risky,
            Threshold::Never => FailOn::Never,
        }
    }
}

/// Where the workspace and its configuration are.
#[derive(Debug, Args)]
pub(crate) struct WorkspaceArgs {
    /// Workspace root; references may not leave it (default: the current
    /// directory, or `[workspace] root` in the configuration).
    #[arg(long)]
    root: Option<PathBuf>,
    /// Configuration file (default: faultline.toml in the root, if present).
    #[arg(long)]
    config: Option<PathBuf>,
}

/// Options shared by commands that load two versions.
#[derive(Debug, Args)]
pub(crate) struct CompareArgs {
    /// The old version's entry document.
    old: PathBuf,
    /// The new version's entry document (default: the same path as OLD,
    /// which is useful with --old-rev).
    new: Option<PathBuf>,
    /// Read the old version from this git revision instead of the working
    /// tree.
    #[arg(long, value_name = "REV")]
    old_rev: Option<String>,
    /// Read the new version from this git revision instead of the working
    /// tree.
    #[arg(long, value_name = "REV")]
    new_rev: Option<String>,
    /// API name used in reports and finding ids (default: the file stem).
    #[arg(long)]
    name: Option<String>,
    /// Description format (default: detected).
    #[arg(long, value_enum)]
    format: Option<Format>,
    /// The old version's format, when it differs from the new one's.
    #[arg(long, value_enum)]
    old_format: Option<Format>,
    /// The new version's format, when it differs from the old one's: for
    /// example `--new-format protobuf-http` to check a gRPC service against
    /// its OpenAPI description.
    #[arg(long, value_enum)]
    new_format: Option<Format>,
    /// For schemas without operations (JSON Schema, Protobuf messages,
    /// Avro): which direction documents flow. For Avro, `input` is BACKWARD
    /// compatibility, `output` FORWARD and `both` FULL.
    #[arg(long, value_enum, default_value = "both")]
    role: Role,
    /// An overlay to apply to both versions before comparing them; may be
    /// repeated, and the overlays apply in order.
    #[arg(long = "overlay", value_name = "FILE")]
    overlays: Vec<PathBuf>,
    #[command(flatten)]
    workspace: WorkspaceArgs,
}

/// Options that decide and present the verdict.
#[derive(Debug, Args)]
pub(crate) struct GateArgs {
    /// Output format.
    #[arg(long, value_enum, default_value = "text")]
    output: Output,
    /// Fail when a finding is at least this severe (overrides configuration).
    #[arg(long, value_enum)]
    fail_on: Option<Threshold>,
    /// Baseline of accepted findings (overrides configuration).
    #[arg(long)]
    baseline: Option<PathBuf>,
    /// Also write the report in another format to a file, such as
    /// `sarif=faultline.sarif`; may be repeated.
    #[arg(long = "report", value_name = "FORMAT=FILE", value_parser = parse_report)]
    reports: Vec<(Output, PathBuf)>,
}

/// Parses a `--report` value, `FORMAT=FILE`.
fn parse_report(value: &str) -> Result<(Output, PathBuf), String> {
    let (format, path) = value.split_once('=').ok_or_else(|| {
        format!("expected FORMAT=FILE, such as sarif=faultline.sarif, not `{value}`")
    })?;
    if path.is_empty() {
        return Err(format!("no file named for the {format} report"));
    }
    let format = <Output as clap::ValueEnum>::from_str(format, true).map_err(|_| {
        let known: Vec<String> = <Output as clap::ValueEnum>::value_variants()
            .iter()
            .filter_map(|v| v.to_possible_value().map(|p| p.get_name().to_owned()))
            .collect();
        format!(
            "unknown report format `{format}`; expected one of {}",
            known.join(", ")
        )
    })?;
    Ok((format, PathBuf::from(path)))
}

#[derive(Debug, Args)]
pub(crate) struct DiffArgs {
    #[command(flatten)]
    compare: CompareArgs,
    #[command(flatten)]
    gate: GateArgs,
}

#[derive(Debug, Args)]
pub(crate) struct CheckArgs {
    /// The revision to compare with, such as origin/main (default: the
    /// pull or merge request's base in GitHub Actions and GitLab CI).
    #[arg(long, value_name = "REV")]
    base: Option<String>,
    /// Compare this revision instead of the working tree.
    #[arg(long, value_name = "REV")]
    head: Option<String>,
    /// Compare with BASE itself rather than with the merge base of BASE and
    /// the head, which also reports changes made on BASE since the head
    /// branched from it.
    #[arg(long)]
    no_merge_base: bool,
    /// Check only these APIs (by name; repeatable).
    #[arg(long = "api", value_name = "NAME")]
    apis: Vec<String>,
    #[command(flatten)]
    workspace: WorkspaceArgs,
    #[command(flatten)]
    gate: GateArgs,
}

#[derive(Debug, Args)]
pub(crate) struct BaselineArgs {
    #[command(flatten)]
    compare: CompareArgs,
    /// Where to write the baseline.
    #[arg(long, default_value = ".faultline/baseline.json")]
    out: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct UsageImportArgs {
    /// The API description the usage refers to (its current version).
    api: PathBuf,
    /// Server access logs (Common or Combined Log Format, or JSON lines).
    #[arg(long = "access-log", value_name = "FILE", num_args = 1.., required_unless_present_any = ["graphql", "har"], conflicts_with_all = ["graphql", "har"])]
    access_logs: Vec<PathBuf>,
    /// GraphQL documents clients send.
    #[arg(long = "graphql", value_name = "FILE", num_args = 1.., conflicts_with = "har")]
    graphql: Vec<PathBuf>,
    /// Recorded traffic as HTTP Archives (HAR), which also show the request
    /// fields clients send.
    #[arg(long = "har", value_name = "FILE", num_args = 1..)]
    har: Vec<PathBuf>,
    /// A path prefix the API is served under (`/v1`), removed before
    /// matching access log requests.
    #[arg(long)]
    base_path: Option<String>,
    /// A description of the period or corpus covered, stored in the index.
    #[arg(long)]
    window: Option<String>,
    /// Add to an existing index instead of replacing it.
    #[arg(long)]
    merge: bool,
    /// Where to write the index.
    #[arg(long, default_value = ".faultline/usage.json")]
    out: PathBuf,
    #[arg(long, value_enum)]
    format: Option<Format>,
    #[command(flatten)]
    workspace: WorkspaceArgs,
}

#[derive(Debug, Args)]
pub(crate) struct DumpArgs {
    /// Entry document.
    file: PathBuf,
    /// Description format (default: detected).
    #[arg(long, value_enum)]
    format: Option<Format>,
    /// For schemas without operations: which direction documents flow.
    #[arg(long, value_enum, default_value = "both")]
    role: Role,
    /// Read the description from this git revision instead of the working
    /// tree.
    #[arg(long, value_name = "REV")]
    rev: Option<String>,
    /// An overlay to apply before lowering; may be repeated, and the
    /// overlays apply in order.
    #[arg(long = "overlay", value_name = "FILE")]
    overlays: Vec<PathBuf>,
    /// Workspace root (default: the current directory).
    #[arg(long)]
    root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct LspArgs {
    /// Revision to compare with, unless the editor chooses one in its
    /// initialization options.
    #[arg(long, default_value = faultline_lsp::DEFAULT_BASE)]
    base: String,
}

/// Runs a command and returns the process exit code.
pub(crate) fn run(command: Command) -> i32 {
    let result = match command {
        Command::Diff(args) => diff(&args),
        Command::Check(args) => check(&args),
        Command::Init(args) => crate::init::init(&args),
        Command::Baseline(args) => baseline(&args),
        Command::Explain { rule } => explain(&rule),
        Command::Rules { markdown, json } => {
            print!(
                "{}",
                if markdown {
                    rules_markdown()
                } else if json {
                    rules_json()
                } else {
                    rules_table()
                }
            );
            Ok(exit::PASSED)
        }
        Command::Ir {
            command: IrCommand::Dump(args),
        } => dump(&args),
        Command::Usage {
            command: UsageCommand::Import(args),
        } => usage_import(&args),
        Command::Lsp(args) => Ok(lsp(&args)),
        Command::Registry { command } => crate::registry::run(&command),
        Command::Completions { shell } => {
            let mut cli = <crate::Cli as clap::CommandFactory>::command();
            clap_complete::generate(shell, &mut cli, "faultline", &mut std::io::stdout());
            Ok(exit::PASSED)
        }
    };
    result.unwrap_or_else(|message| {
        eprintln!("error: {message}");
        exit::INVALID_INPUT
    })
}

fn lsp(args: &LspArgs) -> i32 {
    let server = faultline_lsp::Server::new()
        .with_base(&args.base)
        .with_today(today());
    faultline_lsp::run(
        std::io::BufReader::new(std::io::stdin()),
        std::io::stdout().lock(),
        server,
    )
}

/// Loads the configuration and resolves the workspace root.
fn setup(args: &WorkspaceArgs) -> Result<(Config, PathBuf), String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("cannot read the current directory: {e}"))?;
    let default_root = args.root.clone().unwrap_or_else(|| cwd.clone());
    let config_path = args
        .config
        .clone()
        .or_else(|| Some(default_root.join("faultline.toml")).filter(|p| p.is_file()));
    let config = match &config_path {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            Config::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None => Config::default(),
    };
    let root = match (&args.root, &config.root, &config_path) {
        (Some(root), _, _) => root.clone(),
        (None, Some(root), Some(path)) => path.parent().unwrap_or(Path::new(".")).join(root),
        _ => cwd,
    };
    Ok((config, root))
}

fn target(args: &CompareArgs, root: &Path) -> Target {
    let new = args.new.as_ref().unwrap_or(&args.old);
    let mut old_source = Source::new(root, &args.old).with_overlays(args.overlays.clone());
    if let Some(rev) = &args.old_rev {
        old_source = old_source.at(Revision::Named(rev.clone()));
    }
    let mut new_source = Source::new(root, new).with_overlays(args.overlays.clone());
    if let Some(rev) = &args.new_rev {
        new_source = new_source.at(Revision::Named(rev.clone()));
    }
    let mut target = Target::new(
        args.name.clone().unwrap_or_else(|| default_api_name(new)),
        old_source,
        new_source,
    );
    target.format = args.format.map(Into::into);
    target.old_format = args.old_format.map(Into::into);
    target.new_format = args.new_format.map(Into::into);
    target.role = args.role.into();
    target
}

/// An engine following the configuration's network policy.
fn engine(config: &Config) -> Result<Engine, String> {
    Engine::default()
        .with_network(&config.network)
        .map_err(|e| format!("invalid configuration: {e}"))
}

fn today() -> Date {
    // Reproducible runs (and tests of waiver expiry) can pin the date.
    std::env::var("FAULTLINE_TODAY")
        .ok()
        .and_then(|d| Date::parse(&d))
        .unwrap_or_else(Date::today)
}

fn diff(args: &DiffArgs) -> Result<i32, String> {
    let (config, root) = setup(&args.compare.workspace)?;
    gate(config, &root, &args.gate, &[target(&args.compare, &root)])
}

/// The base of the pull or merge request being built, from the CI
/// environment: the target branch in GitHub Actions, and in GitLab CI the
/// commit the merge request's diff starts from (or, failing that, the
/// target branch).
fn ci_base() -> Option<String> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    var("GITHUB_BASE_REF")
        .map(|branch| format!("origin/{branch}"))
        .or_else(|| var("CI_MERGE_REQUEST_DIFF_BASE_SHA"))
        .or_else(|| var("CI_MERGE_REQUEST_TARGET_BRANCH_NAME").map(|b| format!("origin/{b}")))
}

/// Compares every configured API between a base revision and the working
/// tree (or a head revision).
fn check(args: &CheckArgs) -> Result<i32, String> {
    let (config, root) = setup(&args.workspace)?;
    let Some(base) = args.base.clone().or_else(ci_base) else {
        return Err("no base revision; pass --base (for example --base origin/main)".to_owned());
    };
    let old_revision = if args.no_merge_base {
        Revision::Named(base)
    } else {
        Revision::MergeBase {
            base,
            head: args.head.clone().unwrap_or_else(|| "HEAD".to_owned()),
        }
    };
    if config.apis.is_empty() {
        return Err(
            "the configuration defines no [[api]] entries; `faultline check` compares those \
             (see docs/configuration.md)"
                .to_owned(),
        );
    }
    for name in &args.apis {
        if !config.apis.iter().any(|api| &api.name == name) {
            return Err(format!("no [[api]] named {name} in the configuration"));
        }
    }
    let targets: Vec<Target> = config
        .apis
        .iter()
        .filter(|api| args.apis.is_empty() || args.apis.contains(&api.name))
        .map(|api| {
            let old = Source::new(&root, &api.entry)
                .at(old_revision.clone())
                .with_overlays(api.overlays.clone());
            let mut new = Source::new(&root, &api.entry).with_overlays(api.overlays.clone());
            if let Some(head) = &args.head {
                new = new.at(Revision::Named(head.clone()));
            }
            let mut target = Target::new(&api.name, old, new);
            target.format = api.format;
            target.role = api.role;
            target.may_be_new = true;
            target
        })
        .collect();
    gate(config, &root, &args.gate, &targets)
}

/// Compares `targets`, applies the policy and prints the report.
fn gate(
    mut config: Config,
    root: &Path,
    args: &GateArgs,
    targets: &[Target],
) -> Result<i32, String> {
    if let Some(fail_on) = args.fail_on {
        config.fail_on = fail_on.into();
    }
    let baseline_path = args
        .baseline
        .clone()
        .or_else(|| config.baseline.as_ref().map(|p| root.join(p)));
    let baseline = match baseline_path.filter(|p| args.baseline.is_some() || p.is_file()) {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            Some(Baseline::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?)
        }
        None => None,
    };

    let comparison = engine(&config)?.compare(targets);
    let mut evaluation = evaluate(&config, baseline.as_ref(), comparison.findings, today());
    let releases: Vec<Release> = comparison
        .apis
        .iter()
        .map(|api| Release {
            api: api.name.clone(),
            old_version: api.old_version.clone(),
            new_version: api.new_version.clone(),
            required: api.bump,
        })
        .collect();
    evaluation.check_releases(config.versioning, &releases);
    if let Some(path) = &config.usage.index {
        let path = root.join(path);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let index = faultline_usage::UsageIndex::parse(&text)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        faultline_usage::apply(&mut evaluation, &index, config.usage.unused_severity);
    }
    let report = Report {
        sources: &comparison.sources,
        diagnostics: &comparison.diagnostics,
        apis: &comparison.apis,
        evaluation: &evaluation,
    };
    for (format, path) in &args.reports {
        std::fs::write(path, render(&report, (*format).into()))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    print!("{}", render(&report, args.output.into()));
    Ok(if comparison.diagnostics.has_errors() {
        exit::INVALID_INPUT
    } else {
        evaluation.exit_code()
    })
}

fn baseline(args: &BaselineArgs) -> Result<i32, String> {
    let (config, root) = setup(&args.compare.workspace)?;
    let comparison = engine(&config)?.compare(&[target(&args.compare, &root)]);
    if comparison.diagnostics.has_errors() {
        for d in &comparison.diagnostics {
            eprint!(
                "{}",
                faultline_source::render::render(d, &comparison.sources)
            );
        }
        return Ok(exit::INVALID_INPUT);
    }
    let baseline = Baseline::from_findings(&comparison.findings);
    let out = if args.out.is_absolute() {
        args.out.clone()
    } else {
        root.join(&args.out)
    };
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&out, baseline.to_json())
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    eprintln!(
        "wrote {} findings to {}",
        baseline.findings.len(),
        out.display()
    );
    Ok(exit::PASSED)
}

fn explain(code: &str) -> Result<i32, String> {
    if code.len() > 3 && code[..3].eq_ignore_ascii_case("FLE") {
        let (code, krate, meaning) = crate::diagnostics::DIAGNOSTICS
            .iter()
            .find(|(c, _, _)| c.eq_ignore_ascii_case(code))
            .ok_or_else(|| {
                format!("no diagnostic has the code {code}; docs/diagnostics.md lists them")
            })?;
        println!("{code} ({krate})\n\n{meaning}");
        return Ok(exit::PASSED);
    }
    let rule =
        faultline_rules::catalog::find(code).ok_or_else(
            || match faultline_rules::catalog::suggest(code) {
                Some(rule) => format!(
                    "no rule named {code}; did you mean {} ({})?",
                    rule.name, rule.code
                ),
                None => format!("no rule named {code}; `faultline rules` lists them"),
            },
        )?;
    println!("{} {}: {}\n", rule.code, rule.name, rule.title);
    println!("{}\n", rule.explanation);
    println!("dimension: {}", rule.dimension.as_str());
    Ok(exit::PASSED)
}

fn rules_table() -> String {
    let mut out = String::new();
    for rule in faultline_rules::RULES {
        let _ = writeln!(out, "{}  {:<36} {}", rule.code, rule.name, rule.title);
    }
    out
}

/// The rule reference in Markdown, as committed in `docs/rules.md`.
/// The rule catalog as JSON: each rule's code, name, dimension, summary,
/// Markdown explanation and documentation page.
fn rules_json() -> String {
    let rules: Vec<serde_json::Value> = faultline_rules::RULES
        .iter()
        .map(|rule| {
            serde_json::json!({
                "code": rule.code,
                "name": rule.name,
                "dimension": rule.dimension.as_str(),
                "summary": rule.title,
                "explanation": rule.explanation,
                "help_uri": format!(
                    "{}/blob/main/docs/rules.md#{}",
                    faultline_report::TOOL_URI,
                    rule.code.to_ascii_lowercase()
                ),
            })
        })
        .collect();
    let mut out = serde_json::to_string_pretty(&serde_json::json!({ "rules": rules }))
        .expect("the catalog serializes");
    out.push('\n');
    out
}

pub(crate) fn rules_markdown() -> String {
    let mut out = String::from(
        "# Rules\n\n\
         <!-- Generated by `faultline rules --markdown`; do not edit by hand. -->\n\n\
         Every finding Faultline reports belongs to one of these rules. Severity is\n\
         not fixed per rule: it depends on whether values flow into or out of the API\n\
         at the change, and on the evidence found (see\n\
         [comparison internals](internals/comparison.md)).\n\n\
         | Code | Name | Dimension | Summary |\n|---|---|---|---|\n",
    );
    for rule in faultline_rules::RULES {
        let _ = writeln!(
            out,
            "| [{code}](#{anchor}) | `{}` | {} | {} |",
            rule.name,
            rule.dimension.as_str(),
            rule.title,
            code = rule.code,
            anchor = rule.code.to_ascii_lowercase()
        );
    }
    for rule in faultline_rules::RULES {
        let _ = write!(
            out,
            "\n## {}\n\n**{}** — {}\n\n{}\n",
            rule.code, rule.name, rule.title, rule.explanation
        );
    }
    out
}

/// Builds a usage index from access logs or GraphQL documents.
fn usage_import(args: &UsageImportArgs) -> Result<i32, String> {
    use faultline_usage::{UsageIndex, access_log, graphql, har};
    use std::io::BufRead as _;

    let (config, root) = setup(&args.workspace)?;
    let sources = SourceMap::new();
    let lowered = engine(&config)?.lower(
        &Source::new(&root, &args.api),
        &default_api_name(&args.api),
        args.format.map(Into::into),
        SchemaRole::Both,
        &sources,
    );
    for d in &lowered.diagnostics {
        eprint!("{}", faultline_source::render::render(d, &sources));
    }
    let Some(api) = lowered.api.filter(|_| !lowered.diagnostics.has_errors()) else {
        return Ok(exit::INVALID_INPUT);
    };
    let open = |path: &PathBuf| {
        std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    };
    let options = access_log::AccessLogOptions {
        base_path: args.base_path.clone(),
    };
    let mut index = if !args.har.is_empty() {
        let mut index = UsageIndex::new(faultline_usage::Source::Har);
        for path in &args.har {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            index.merge(
                &har::import(&api, &text, &options)
                    .map_err(|e| format!("{}: {e}", path.display()))?,
            );
        }
        index
    } else if args.graphql.is_empty() {
        let mut lines = Vec::new();
        let mut index = UsageIndex::new(faultline_usage::Source::AccessLog);
        for path in &args.access_logs {
            // Stream the log in chunks so large logs are not held in memory.
            for line in std::io::BufReader::new(open(path)?).lines() {
                lines.push(line.map_err(|e| format!("cannot read {}: {e}", path.display()))?);
                if lines.len() == 100_000 {
                    index.merge(&access_log::import(&api, lines.drain(..), &options));
                }
            }
        }
        index.merge(&access_log::import(&api, lines, &options));
        index
    } else {
        let mut documents = Vec::new();
        for path in &args.graphql {
            documents.push(
                std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?,
            );
        }
        graphql::import(&api, &documents)
    };
    index.window.clone_from(&args.window);
    let out = if args.out.is_absolute() {
        args.out.clone()
    } else {
        root.join(&args.out)
    };
    if args.merge && out.is_file() {
        let text = std::fs::read_to_string(&out)
            .map_err(|e| format!("cannot read {}: {e}", out.display()))?;
        let mut existing =
            UsageIndex::parse(&text).map_err(|e| format!("{}: {e}", out.display()))?;
        if existing.source != index.source {
            return Err(format!(
                "{} holds {:?} usage and cannot be merged with {:?} usage",
                out.display(),
                existing.source,
                index.source
            ));
        }
        existing.merge(&index);
        index = existing;
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&out, index.to_json())
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    let calls: u64 = index.operations.values().map(|o| o.calls).sum();
    let unused = index.operations.values().filter(|o| o.calls == 0).count();
    eprintln!(
        "wrote usage of {} operations ({calls} calls, {unused} unused, {} unmatched) to {}",
        index.operations.len(),
        index.unmatched,
        out.display()
    );
    Ok(exit::PASSED)
}

fn dump(args: &DumpArgs) -> Result<i32, String> {
    let root = match &args.root {
        Some(root) => root.clone(),
        None => std::env::current_dir()
            .map_err(|e| format!("cannot read the current directory: {e}"))?,
    };
    let sources = SourceMap::new();
    let mut source = Source::new(&root, &args.file).with_overlays(args.overlays.clone());
    if let Some(rev) = &args.rev {
        source = source.at(Revision::Named(rev.clone()));
    }
    let lowered = Engine::default().lower(
        &source,
        &default_api_name(&args.file),
        args.format.map(Into::into),
        args.role.into(),
        &sources,
    );
    for d in &lowered.diagnostics {
        eprint!("{}", faultline_source::render::render(d, &sources));
    }
    match lowered.api {
        Some(api) if !lowered.diagnostics.has_errors() => {
            print!("{}", faultline_ir::dump(&api));
            Ok(exit::PASSED)
        }
        _ => Ok(exit::INVALID_INPUT),
    }
}
