//! `faultline init`: a `faultline.toml` listing the API descriptions found
//! in a workspace.
//!
//! Files are chosen by what they hold, not only their names:
//!
//! * JSON and YAML documents declaring `openapi`, `swagger` or `asyncapi`;
//! * RAML API documents (not fragments such as libraries);
//! * Protobuf, Thrift and Smithy files that declare a service;
//! * GraphQL schemas that define `Query` or a `schema`;
//! * Avro schemas, protocols and IDL files, WSDL descriptions and JSON Type
//!   Definitions.
//!
//! XML Schemas are left out, since most are included by others; add their
//! entry documents by hand. Hidden directories and dependency or build
//! output (`node_modules`, `target`, `vendor`, ...) are not searched.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clap::Args;

/// Directories never searched.
const SKIPPED: [&str; 8] = [
    "node_modules",
    "target",
    "vendor",
    "build",
    "dist",
    "out",
    "third_party",
    "__pycache__",
];

/// Directories are searched at most this deep, and at most this many files
/// are looked at.
const MAX_DEPTH: usize = 12;
const MAX_FILES: usize = 50_000;
/// Only the start of a document is read to recognize it.
const SNIFF_BYTES: usize = 16 * 1024;

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// The workspace to search (default: the current directory).
    #[arg(long)]
    root: Option<PathBuf>,
    /// Print the configuration instead of writing faultline.toml.
    #[arg(long)]
    print: bool,
    /// Replace an existing faultline.toml.
    #[arg(long)]
    force: bool,
}

/// An API description found in the workspace.
#[derive(Debug, PartialEq, Eq)]
struct Found {
    /// Relative to the root, with `/` separators.
    path: String,
    /// What it was recognized as, for the summary.
    kind: &'static str,
}

pub(crate) fn init(args: &InitArgs) -> Result<i32, String> {
    let root = match &args.root {
        Some(root) => root.clone(),
        None => std::env::current_dir()
            .map_err(|e| format!("cannot read the current directory: {e}"))?,
    };
    let target = root.join("faultline.toml");
    if !args.print && target.exists() && !args.force {
        return Err(format!(
            "{} exists; pass --force to replace it, or --print to see what would be written",
            target.display()
        ));
    }
    let mut found = Vec::new();
    let mut visited = 0;
    search(&root, &root, 0, &mut visited, &mut found);
    found.sort_by(|a, b| a.path.cmp(&b.path));
    let found = without_included(&root, found);
    let config = render(&found);
    if args.print {
        print!("{config}");
    } else {
        std::fs::write(&target, &config)
            .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
        eprintln!("wrote {} with {} APIs", target.display(), found.len());
    }
    for api in &found {
        eprintln!("  {} ({})", api.path, api.kind);
    }
    if found.is_empty() {
        eprintln!("no API descriptions found; add [[api]] entries by hand");
    }
    Ok(0)
}

fn search(root: &Path, dir: &Path, depth: usize, visited: &mut usize, found: &mut Vec<Found>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if *visited >= MAX_FILES {
            return;
        }
        *visited += 1;
        let name = entry.file_name().to_string_lossy().into_owned();
        // Symbolic links are not followed: they may lead out of the root.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            if !name.starts_with('.') && !SKIPPED.contains(&name.as_str()) {
                search(root, &path, depth + 1, visited, found);
            }
        } else if file_type.is_file()
            && let Some(kind) = recognize(&path, &name)
        {
            let relative: Vec<String> = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            found.push(Found {
                path: relative.join("/"),
                kind,
            });
        }
    }
}

/// What kind of API description the file at `path` is, if it is one.
fn recognize(path: &Path, name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".jtd.json") {
        return Some("JSON Type Definition");
    }
    let extension = lower.rsplit_once('.').map_or("", |(_, e)| e);
    let head = || sniff(path);
    match extension {
        "yaml" | "yml" | "json" => {
            let head = head()?;
            let declares = |key: &str| {
                head.lines().any(|line| {
                    let line = line.trim_start().trim_start_matches(['{', ' ']);
                    line.starts_with(&format!("{key}:")) || line.starts_with(&format!("\"{key}\""))
                })
            };
            if declares("openapi") {
                Some("OpenAPI")
            } else if declares("swagger") {
                Some("Swagger")
            } else if declares("asyncapi") {
                Some("AsyncAPI")
            } else if declares("smithy") && declares("shapes") {
                Some("Smithy JSON AST")
            } else {
                None
            }
        }
        "raml" => {
            let first = head()?.lines().next()?.trim().to_owned();
            (first == "#%RAML 1.0").then_some("RAML")
        }
        "proto" => declares_service(&head()?).then_some("Protobuf"),
        "thrift" => declares_service(&head()?).then_some("Thrift"),
        "smithy" => declares_service(&head()?).then_some("Smithy"),
        "graphql" | "graphqls" | "gql" => {
            let head = head()?;
            (head.contains("type Query") || head.contains("schema {") || head.contains("schema{"))
                .then_some("GraphQL")
        }
        "avsc" | "avpr" | "avdl" => Some("Avro"),
        "wsdl" => Some("WSDL"),
        _ => None,
    }
}

/// `found` without the files that others include or import (Thrift
/// `include`, Avro IDL `import`), which are part of those APIs rather than
/// APIs of their own.
fn without_included(root: &Path, found: Vec<Found>) -> Vec<Found> {
    let mut included = std::collections::HashSet::new();
    for api in &found {
        let extension = Path::new(&api.path)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        if !matches!(extension.as_deref(), Some("thrift" | "avdl")) {
            continue;
        }
        let Some(text) = sniff(&root.join(&api.path)) else {
            continue;
        };
        let dir = api.path.rsplit_once('/').map_or("", |(d, _)| d);
        for line in text.lines() {
            let line = line.trim_start();
            let rest = line
                .strip_prefix("include ")
                .or_else(|| line.strip_prefix("import idl "))
                .or_else(|| line.strip_prefix("import protocol "))
                .or_else(|| line.strip_prefix("import schema "));
            let Some(path) = rest.and_then(|r| r.split('"').nth(1)) else {
                continue;
            };
            if let Some(joined) = join(dir, path) {
                included.insert(joined);
            }
        }
    }
    found
        .into_iter()
        .filter(|api| !included.contains(&api.path))
        .collect()
}

/// `path` relative to the directory `dir`, both with `/` separators, with
/// `.` and `..` resolved; `None` if it leaves the root.
fn join(dir: &str, path: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// Whether a line starts with `service`.
fn declares_service(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_start().strip_prefix("service ").is_some())
}

/// The start of a file, as text.
fn sniff(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(SNIFF_BYTES);
    file.take(SNIFF_BYTES as u64).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The configuration listing `found`, with names from file stems, made
/// unique with their directories where they clash.
fn render(found: &[Found]) -> String {
    let stem = |path: &str| {
        let file = path.rsplit('/').next().unwrap_or(path);
        let file = file.strip_suffix(".jtd.json").unwrap_or(file);
        file.rsplit_once('.').map_or(file, |(s, _)| s).to_owned()
    };
    let mut out = String::from(
        "# API descriptions compared by `faultline check`. Review the list: remove\n\
         # descriptions that are not published APIs, and set `role` for schemas\n\
         # without operations (input, output or both).\n",
    );
    for api in found {
        let mut name = stem(&api.path);
        if found
            .iter()
            .filter(|other| stem(&other.path) == name)
            .count()
            > 1
        {
            let dir = api.path.rsplit_once('/').map_or("", |(d, _)| d);
            if !dir.is_empty() {
                name = format!("{}-{name}", dir.replace('/', "-"));
            }
        }
        let _ = write!(
            out,
            "\n[[api]]\nname = {}\nentry = {}\n",
            toml_string(&name),
            toml_string(&api.path)
        );
    }
    out.push_str(
        "\n[policy]\n\
         fail_on = \"breaking\"            # breaking | risky | never\n\
         # versioning = \"semver\"         # check the versions the descriptions declare\n",
    );
    out
}

/// A TOML basic string.
fn toml_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_strings_escaped() {
        let found = [
            Found {
                path: "api/public/openapi.yaml".into(),
                kind: "OpenAPI",
            },
            Found {
                path: "api/admin/openapi.yaml".into(),
                kind: "OpenAPI",
            },
            Found {
                path: "events/user.jtd.json".into(),
                kind: "JSON Type Definition",
            },
        ];
        let config = render(&found);
        assert!(config.contains("name = \"api-public-openapi\""), "{config}");
        assert!(config.contains("name = \"api-admin-openapi\""), "{config}");
        assert!(config.contains("name = \"user\""), "{config}");
        assert_eq!(toml_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000A\"");
        assert_eq!(
            join("idl/v1", "../common/money.avdl").as_deref(),
            Some("idl/common/money.avdl")
        );
        assert_eq!(join("", "../x.thrift"), None);
        faultline_policy::Config::parse(&config).expect("the configuration is valid");
    }
}
