//! The `faultline` command-line tool.

mod commands;
mod diagnostics;
mod init;
mod registry;

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
    /// Write a faultline.toml listing the API descriptions in a workspace.
    Init(init::InitArgs),
    /// Record the current findings between two versions as accepted.
    Baseline(commands::BaselineArgs),
    /// Explain a rule or a diagnostic code.
    Explain {
        /// Rule code (FL1007) or name (bound-changed), or a diagnostic code
        /// (FLE7605).
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
    /// Build usage indexes from access logs or client documents.
    Usage {
        #[command(subcommand)]
        command: UsageCommand,
    },
    /// Serve the Language Server Protocol over standard input and output,
    /// for editors.
    Lsp(commands::LspArgs),
    /// Run or use a schema registry that refuses breaking versions.
    Registry {
        #[command(subcommand)]
        command: registry::RegistryCommand,
    },
}

#[derive(Debug, Subcommand)]
enum UsageCommand {
    /// Record which operations, parameters and fields clients use.
    Import(commands::UsageImportArgs),
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
    /// GitLab Code Quality.
    Gitlab,
    /// A self-contained HTML page.
    Html,
    /// Release notes in Markdown.
    Changelog,
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
            Output::Gitlab => Self::Gitlab,
            Output::Html => Self::Html,
            Output::Changelog => Self::Changelog,
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
