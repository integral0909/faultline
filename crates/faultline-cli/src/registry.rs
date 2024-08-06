//! `faultline registry`: serving a registry and sending versions to one.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Subcommand};
use faultline::Engine;
use faultline_policy::{Date, exit};
use faultline_registry::http::{Options, Server};
use faultline_registry::{Client, Compatibility, Registry, Reply, Store, bundle};
use serde_json::Value;

use crate::commands::{Format, Role, Threshold};

/// The environment variable holding the bearer token for writes, on both
/// the server and the client.
const TOKEN_VARIABLE: &str = "FAULTLINE_REGISTRY_TOKEN";

#[derive(Debug, Subcommand)]
pub(crate) enum RegistryCommand {
    /// Serve a registry over HTTP.
    Serve(ServeArgs),
    /// Register the next version of an API description.
    Push(SendArgs),
    /// Check a version against a registry without registering it.
    Check(SendArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ServeArgs {
    /// Directory holding the registered versions (created if needed).
    #[arg(long)]
    dir: PathBuf,
    /// Address to listen on. The default accepts local connections only;
    /// serve other hosts through a TLS-terminating proxy.
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: String,
    /// Default for subjects without settings: findings of this severity
    /// refuse a version.
    #[arg(long, value_enum, default_value = "breaking")]
    fail_on: Threshold,
    /// Default for subjects without settings: check against every earlier
    /// version.
    #[arg(long)]
    transitive: bool,
    /// Requests served at once.
    #[arg(long, default_value_t = 8)]
    threads: usize,
    /// The largest request body accepted, in bytes.
    #[arg(long, default_value_t = 16 * 1024 * 1024)]
    max_body_bytes: usize,
}

#[derive(Debug, Args)]
pub(crate) struct SendArgs {
    /// The registry's URL, e.g. `https://registry.example.com`.
    registry: String,
    /// The subject to register under.
    subject: String,
    /// Entry document; it and every workspace file it references are sent.
    entry: PathBuf,
    /// Workspace root (default: the current directory).
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long, value_enum)]
    format: Option<Format>,
    #[arg(long, value_enum, default_value = "both")]
    role: Role,
    /// Print the registry's JSON response instead of a summary.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(command: &RegistryCommand) -> Result<i32, String> {
    match command {
        RegistryCommand::Serve(args) => serve(args),
        RegistryCommand::Push(args) => send(args, true),
        RegistryCommand::Check(args) => send(args, false),
    }
}

fn token() -> Option<String> {
    std::env::var(TOKEN_VARIABLE).ok().filter(|t| !t.is_empty())
}

fn serve(args: &ServeArgs) -> Result<i32, String> {
    let store = Store::open(&args.dir)
        .map_err(|e| format!("cannot use {} for the registry: {e}", args.dir.display()))?;
    let token = token();
    if token.is_none() {
        eprintln!(
            "warning: {TOKEN_VARIABLE} is not set, so anyone who can connect may register versions"
        );
    }
    let mut registry = Registry::new(store)
        .with_defaults(Compatibility {
            fail_on: args.fail_on.into(),
            transitive: args.transitive,
        })
        .with_token(token);
    // A long-running server follows the clock unless a date is pinned.
    if let Some(date) = std::env::var("FAULTLINE_TODAY")
        .ok()
        .and_then(|d| Date::parse(&d))
    {
        registry = registry.with_today(date);
    }
    let options = Options {
        threads: args.threads.max(1),
        max_body_bytes: args.max_body_bytes,
        timeout: Duration::from_secs(30),
        ..Options::default()
    };
    let server = Server::start(args.listen.as_str(), options, &registry.into_handler())
        .map_err(|e| format!("cannot listen on {}: {e}", args.listen))?;
    eprintln!(
        "faultline registry listening on http://{}",
        server.address()
    );
    server.wait();
    Ok(exit::PASSED)
}

fn send(args: &SendArgs, register: bool) -> Result<i32, String> {
    let root = match &args.root {
        Some(root) => root.clone(),
        None => std::env::current_dir()
            .map_err(|e| format!("cannot read the current directory: {e}"))?,
    };
    let bundle = bundle(
        &Engine::default(),
        &root,
        &args.entry,
        args.format.map(Into::into),
        args.role.into(),
    )?;
    let client = Client::new(&args.registry, token())?;
    let reply = if register {
        client.register(&args.subject, &bundle)?
    } else {
        client.check(&args.subject, &bundle)?
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&reply.body).expect("JSON values always serialize")
        );
    } else {
        print!("{}", summarize(&args.subject, &reply, register));
    }
    Ok(exit_code(&reply, register))
}

/// The exit code for a registry's reply, as for local checks: 1 when
/// findings refuse the version, 2 when the request itself failed.
fn exit_code(reply: &Reply, register: bool) -> i32 {
    match reply.status {
        200 | 201 if register || reply.body["compatible"] == true => exit::PASSED,
        200 | 409 => exit::FINDINGS,
        _ => exit::INVALID_INPUT,
    }
}

fn summarize(subject: &str, reply: &Reply, register: bool) -> String {
    let body = &reply.body;
    let mut out = String::new();
    let versions = |value: &Value| {
        value
            .as_array()
            .map(|a| {
                a.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    match reply.status {
        201 => {
            let _ = writeln!(out, "registered {subject} version {}", body["version"]);
        }
        200 if register => {
            let _ = writeln!(
                out,
                "{subject} version {} is identical; nothing registered",
                body["version"]
            );
        }
        200 if body["compatible"] == true => {
            let against = versions(&body["compared_with"]);
            if against.is_empty() {
                let _ = writeln!(out, "{subject} has no versions yet; this one is valid");
            } else {
                let _ = writeln!(out, "compatible with {subject} version {against}");
            }
        }
        200 if body["valid"] == false => {
            let _ = writeln!(out, "the description has errors");
        }
        200 => {
            let _ = writeln!(
                out,
                "incompatible with {subject} version {}",
                versions(&body["compared_with"])
            );
        }
        status => {
            let error = body["error"].as_str().unwrap_or("the request failed");
            let _ = writeln!(out, "refused (HTTP {status}): {error}");
        }
    }
    let report = &body["report"];
    for finding in report["findings"].as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "  {}{} {} {}: {}\n    at {}",
            if finding["failing"] == true { "* " } else { "" },
            finding["severity"].as_str().unwrap_or_default(),
            finding["rule"].as_str().unwrap_or_default(),
            finding["rule_name"].as_str().unwrap_or_default(),
            finding["message"].as_str().unwrap_or_default(),
            finding["path"].as_str().unwrap_or_default(),
        );
    }
    for diagnostic in report["diagnostics"].as_array().into_iter().flatten() {
        let place = diagnostic["location"]
            .as_object()
            .map(|l| format!("{}:{}: ", l["file"].as_str().unwrap_or_default(), l["line"]))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "  {place}{}: {}",
            diagnostic["severity"].as_str().unwrap_or_default(),
            diagnostic["message"].as_str().unwrap_or_default()
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exit_codes() {
        let reply = |status, body| Reply { status, body };
        assert_eq!(exit_code(&reply(201, json!({})), true), 0);
        assert_eq!(exit_code(&reply(200, json!({})), true), 0);
        assert_eq!(exit_code(&reply(409, json!({})), true), 1);
        assert_eq!(exit_code(&reply(422, json!({})), true), 2);
        assert_eq!(exit_code(&reply(401, json!({})), true), 2);
        assert_eq!(
            exit_code(&reply(200, json!({"compatible": true})), false),
            0
        );
        assert_eq!(
            exit_code(&reply(200, json!({"compatible": false})), false),
            1
        );
    }

    #[test]
    fn summaries() {
        let refused = Reply {
            status: 409,
            body: json!({
                "error": "the version is incompatible with version 1",
                "report": {"findings": [{
                    "failing": true, "severity": "breaking", "rule": "FL1007",
                    "rule_name": "bound-changed", "message": "maximum lowered", "path": "schema[pet]",
                }]},
            }),
        };
        let text = summarize("pet", &refused, true);
        assert!(
            text.starts_with("refused (HTTP 409): the version is incompatible"),
            "{text}"
        );
        assert!(
            text.contains("* breaking FL1007 bound-changed: maximum lowered\n    at schema[pet]")
        );
        let created = Reply {
            status: 201,
            body: json!({"version": 3, "report": {}}),
        };
        assert_eq!(
            summarize("pet", &created, true),
            "registered pet version 3\n"
        );
    }
}
