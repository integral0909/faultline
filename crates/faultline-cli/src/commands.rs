use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use faultline::{Engine, Source, Target, default_api_name};
use faultline_policy::{ApiFormat, Baseline, Config, Date, FailOn, SchemaRole, evaluate, exit};
use faultline_report::{Report, render};
use faultline_source::SourceMap;

use crate::{Command, IrCommand, Output};

/// `--format` values.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Format {
    Openapi,
    Jsonschema,
}

impl From<Format> for ApiFormat {
    fn from(format: Format) -> Self {
        match format {
            Format::Openapi => ApiFormat::OpenApi,
            Format::Jsonschema => ApiFormat::JsonSchema,
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

/// Options shared by commands that load two versions.
#[derive(Debug, Args)]
pub(crate) struct CompareArgs {
    /// The old version's entry document.
    old: PathBuf,
    /// The new version's entry document.
    new: PathBuf,
    /// API name used in reports and finding ids (default: the file stem).
    #[arg(long)]
    name: Option<String>,
    /// Description format (default: detected).
    #[arg(long, value_enum)]
    format: Option<Format>,
    /// For JSON Schemas: which direction documents flow.
    #[arg(long, value_enum, default_value = "both")]
    role: Role,
    /// Workspace root; references may not leave it (default: the current
    /// directory, or `[workspace] root` in the configuration).
    #[arg(long)]
    root: Option<PathBuf>,
    /// Configuration file (default: faultline.toml in the root, if present).
    #[arg(long)]
    config: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct DiffArgs {
    #[command(flatten)]
    compare: CompareArgs,
    /// Output format.
    #[arg(long, value_enum, default_value = "text")]
    output: Output,
    /// Fail when a finding is at least this severe (overrides configuration).
    #[arg(long, value_enum)]
    fail_on: Option<Threshold>,
    /// Baseline of accepted findings (overrides configuration).
    #[arg(long)]
    baseline: Option<PathBuf>,
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
pub(crate) struct DumpArgs {
    /// Entry document.
    file: PathBuf,
    #[arg(long, value_enum)]
    format: Option<Format>,
    #[arg(long, value_enum, default_value = "both")]
    role: Role,
    #[arg(long)]
    root: Option<PathBuf>,
}

/// Runs a command and returns the process exit code.
pub(crate) fn run(command: Command) -> i32 {
    let result = match command {
        Command::Diff(args) => diff(&args),
        Command::Baseline(args) => baseline(&args),
        Command::Explain { rule } => explain(&rule),
        Command::Rules { markdown } => {
            print!(
                "{}",
                if markdown {
                    rules_markdown()
                } else {
                    rules_table()
                }
            );
            Ok(exit::PASSED)
        }
        Command::Ir {
            command: IrCommand::Dump(args),
        } => dump(&args),
    };
    result.unwrap_or_else(|message| {
        eprintln!("error: {message}");
        exit::INVALID_INPUT
    })
}

/// Loads the configuration and resolves the workspace root.
fn setup(args: &CompareArgs) -> Result<(Config, PathBuf), String> {
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
    let mut target = Target::new(
        args.name
            .clone()
            .unwrap_or_else(|| default_api_name(&args.new)),
        Source::new(root, &args.old),
        Source::new(root, &args.new),
    );
    target.format = args.format.map(Into::into);
    target.role = args.role.into();
    target
}

fn today() -> Date {
    // Reproducible runs (and tests of waiver expiry) can pin the date.
    std::env::var("FAULTLINE_TODAY")
        .ok()
        .and_then(|d| Date::parse(&d))
        .unwrap_or_else(Date::today)
}

fn diff(args: &DiffArgs) -> Result<i32, String> {
    let (mut config, root) = setup(&args.compare)?;
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

    let comparison = Engine::default().compare(&[target(&args.compare, &root)]);
    let evaluation = evaluate(&config, baseline.as_ref(), comparison.findings, today());
    let report = Report {
        sources: &comparison.sources,
        diagnostics: &comparison.diagnostics,
        apis: &comparison.apis,
        evaluation: &evaluation,
    };
    print!("{}", render(&report, args.output.into()));
    Ok(if comparison.diagnostics.has_errors() {
        exit::INVALID_INPUT
    } else {
        evaluation.exit_code()
    })
}

fn baseline(args: &BaselineArgs) -> Result<i32, String> {
    let (_, root) = setup(&args.compare)?;
    let comparison = Engine::default().compare(&[target(&args.compare, &root)]);
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
    let rule = faultline_rules::catalog::find(code)
        .ok_or_else(|| format!("no rule named {code}; `faultline rules` lists them"))?;
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

fn dump(args: &DumpArgs) -> Result<i32, String> {
    let root = match &args.root {
        Some(root) => root.clone(),
        None => std::env::current_dir()
            .map_err(|e| format!("cannot read the current directory: {e}"))?,
    };
    let sources = SourceMap::new();
    let lowered = Engine::default().lower(
        &Source::new(&root, &args.file),
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
