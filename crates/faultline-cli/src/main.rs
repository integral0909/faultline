//! The `faultline` command-line tool.

mod commands;

use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

/// Find breaking changes in API contracts.
#[derive(Debug, Parser)]
#[command(name = "faultline", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Compare two versions of an API description.
    Diff(commands::DiffArgs),
    /// Compare every API in faultline.toml with a base revision.
    Check(commands::CheckArgs),
    /// Record the current findings between two versions as accepted.
    Baseline(commands::BaselineArgs),
    /// Explain a rule.
    Explain {
        /// Rule code (FL1007) or name (bound-changed).
        rule: String,
    },
    /// List every rule.
    Rules {
        /// Print the rule reference as Markdown.
        #[arg(long)]
        markdown: bool,
    },
    /// Inspect Faultline's model of an API.
    Ir {
        #[command(subcommand)]
        command: IrCommand,
    },
}

#[derive(Debug, Subcommand)]
enum IrCommand {
    /// Print the lowered form of an API description.
    Dump(commands::DumpArgs),
}

/// `--output` values.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Output {
    Text,
    Json,
    Sarif,
    Markdown,
    Github,
    Junit,
}

impl From<Output> for faultline_report::OutputFormat {
    fn from(output: Output) -> Self {
        match output {
            Output::Text => Self::Text,
            Output::Json => Self::Json,
            Output::Sarif => Self::Sarif,
            Output::Markdown => Self::Markdown,
            Output::Github => Self::Github,
            Output::Junit => Self::Junit,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // An internal failure must not look like a finding or an input error to
    // the CI system reading the exit code.
    if let Ok(code) = std::panic::catch_unwind(|| commands::run(cli.command)) {
        ExitCode::from(u8::try_from(code).unwrap_or(3))
    } else {
        eprintln!(
            "error: internal error in faultline; please report it with the inputs that caused it"
        );
        ExitCode::from(3)
    }
}
