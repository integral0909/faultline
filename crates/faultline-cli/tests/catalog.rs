//! The catalog of diagnostic codes: generated from the `DiagnosticCode`
//! constants the crates define, with their documentation, into the table
//! `faultline explain` reads (`src/diagnostics.rs`) and into
//! `docs/diagnostics.md`. Regenerate both with
//! `FAULTLINE_UPDATE_DOCS=1 cargo test -p faultline-cli --test catalog`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A defined code: the crate defining it and its documentation.
struct Entry {
    krate: String,
    doc: String,
}

/// Every `.rs` file under `dir`.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The `FLE` codes defined as constants in `text`, with the doc comments
/// before them.
fn definitions(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut doc: Vec<String> = Vec::new();
    let mut statement = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !statement.is_empty() {
            statement.push(' ');
            statement.push_str(trimmed);
        } else if let Some(text) = trimmed.strip_prefix("///") {
            doc.push(text.trim().to_owned());
            continue;
        } else if (trimmed.starts_with("pub const") || trimmed.starts_with("pub(crate) const"))
            && trimmed.contains(": DiagnosticCode")
        {
            statement.push_str(trimmed);
        } else {
            doc.clear();
            continue;
        }
        if !statement.ends_with(';') {
            continue;
        }
        if let Some(code) = statement
            .split("DiagnosticCode::new(\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .filter(|code| code.starts_with("FLE"))
        {
            out.push((code.to_owned(), doc.join(" ")));
        }
        statement.clear();
        doc.clear();
    }
    out
}

fn catalog() -> BTreeMap<String, Entry> {
    let mut catalog = BTreeMap::new();
    let mut crates: Vec<PathBuf> = std::fs::read_dir(repo().join("crates"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    crates.sort();
    for krate in crates {
        let name = krate.file_name().unwrap().to_string_lossy().into_owned();
        let mut files = Vec::new();
        sources(&krate.join("src"), &mut files);
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            for (code, doc) in definitions(&text) {
                let previous = catalog.insert(
                    code.clone(),
                    Entry {
                        krate: name.clone(),
                        doc,
                    },
                );
                if let Some(previous) = previous {
                    panic!("{code} is defined by both {} and {name}", previous.krate);
                }
            }
        }
    }
    catalog
}

fn rust_table(catalog: &BTreeMap<String, Entry>) -> String {
    let mut out = String::from(
        "//! Every diagnostic code, with the crate raising it and what it means.\n\
         //! Generated from the crates' definitions by\n\
         //! `FAULTLINE_UPDATE_DOCS=1 cargo test -p faultline-cli --test catalog`;\n\
         //! do not edit.\n\n\
         /// `(code, crate, meaning)`, ordered by code.\n\
         #[rustfmt::skip]\n\
         pub(crate) const DIAGNOSTICS: &[(&str, &str, &str)] = &[\n",
    );
    for (code, entry) in catalog {
        let _ = writeln!(out, "    ({code:?}, {:?}, {:?}),", entry.krate, entry.doc);
    }
    out.push_str("];\n");
    out
}

fn markdown(catalog: &BTreeMap<String, Entry>) -> String {
    let mut out = String::from(
        "# Diagnostics\n\n\
         Problems with the inputs (a file that does not parse, a reference that\n\
         resolves to nothing) are reported as diagnostics, distinct from findings\n\
         about compatibility. Each has a code; `faultline explain FLE0000` shows\n\
         what a code means. This reference is generated from the definitions.\n\n\
         | Code | Crate | Meaning |\n|---|---|---|\n",
    );
    for (code, entry) in catalog {
        let _ = writeln!(
            out,
            "| {code} | `{}` | {} |",
            entry.krate,
            entry.doc.replace('|', "\\|")
        );
    }
    out
}

#[test]
fn every_code_is_documented_once() {
    let catalog = catalog();
    assert!(catalog.len() > 100, "{} codes", catalog.len());
    for (code, entry) in &catalog {
        assert!(
            !entry.doc.is_empty(),
            "{code} in {} has no documentation",
            entry.krate
        );
    }
}

#[test]
fn committed_catalog_is_current() {
    let catalog = catalog();
    let update = std::env::var_os("FAULTLINE_UPDATE_DOCS").is_some();
    for (path, generated) in [
        (
            repo().join("crates/faultline-cli/src/diagnostics.rs"),
            rust_table(&catalog),
        ),
        (repo().join("docs/diagnostics.md"), markdown(&catalog)),
    ] {
        if update {
            std::fs::write(&path, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == generated,
            "{} is out of date; regenerate it with FAULTLINE_UPDATE_DOCS=1 cargo test -p faultline-cli --test catalog",
            path.display()
        );
    }
}

#[test]
fn definitions_are_read_across_lines() {
    let text = "/// A thing.\n/// More.\npub const A: DiagnosticCode =\n    DiagnosticCode::new(\"FLE9998\");\nfn x() {}\npub const T: DiagnosticCode = DiagnosticCode::new(\"T0001\");\n";
    assert_eq!(
        definitions(text),
        [("FLE9998".to_owned(), "A thing. More.".to_owned())]
    );
}
