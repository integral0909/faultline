//! Times the pipeline for the front ends that read interface definition
//! languages, on generated descriptions of the same shape: `n` record
//! types that refer to each other in a chain, `n` operations taking and
//! returning them, and a fixed share of fields changed in the new version.
//!
//! Run with `cargo bench -p faultline --bench formats`.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use faultline::{Engine, Source, Target};

/// Writes one version of a description with `n` operations.
type Generator = fn(usize, bool) -> String;

/// Whether record `i`'s field `count` is widened in the new version.
fn changed(i: usize, new: bool) -> bool {
    new && i.is_multiple_of(20)
}

fn protobuf(n: usize, new: bool) -> String {
    let mut out = String::from("syntax = \"proto3\";\npackage bench;\n");
    for i in 0..n {
        let count = if changed(i, new) { "int64" } else { "int32" };
        let _ = writeln!(
            out,
            "message Item{i} {{ string id = 1; {count} count = 2; Item{} next = 3; }}",
            (i + 1) % n
        );
    }
    out.push_str("service Bench {\n");
    for i in 0..n {
        let _ = writeln!(out, "  rpc Get{i}(Item{i}) returns (Item{i});");
    }
    out.push_str("}\n");
    out
}

fn thrift(n: usize, new: bool) -> String {
    let mut out = String::new();
    for i in 0..n {
        let count = if changed(i, new) { "i64" } else { "i32" };
        let _ = writeln!(
            out,
            "struct Item{i} {{ 1: required string id, 2: optional {count} count, 3: optional Item{} next }}",
            (i + 1) % n
        );
    }
    out.push_str("service Bench {\n");
    for i in 0..n {
        let _ = writeln!(out, "  Item{i} get{i}(1: Item{i} item),");
    }
    out.push_str("}\n");
    out
}

fn smithy(n: usize, new: bool) -> String {
    let mut out =
        String::from("$version: \"2\"\nnamespace bench\nservice Bench {\n  operations: [");
    for i in 0..n {
        let _ = write!(out, "Get{i}, ");
    }
    out.push_str("]\n}\n");
    for i in 0..n {
        let count = if changed(i, new) { "Long" } else { "Integer" };
        let _ = writeln!(
            out,
            "structure Item{i} {{\n  @required\n  id: String\n  count: {count}\n  next: Item{}\n}}\noperation Get{i} {{\n  input: Item{i}\n  output: Item{i}\n}}",
            (i + 1) % n
        );
    }
    out
}

fn graphql(n: usize, new: bool) -> String {
    let mut out = String::from("type Query {\n");
    for i in 0..n {
        let _ = writeln!(out, "  item{i}(id: ID!): Item{i}");
    }
    out.push_str("}\n");
    for i in 0..n {
        let count = if changed(i, new) { "Float" } else { "Int" };
        let _ = writeln!(
            out,
            "type Item{i} {{ id: ID! count: {count} next: Item{} }}",
            (i + 1) % n
        );
    }
    out
}

fn main() {
    // `cargo test --benches` runs this without `--bench`; keep that quick.
    let quick = !std::env::args().any(|a| a == "--bench");
    let sizes: &[usize] = if quick { &[20] } else { &[250, 1000, 4000] };
    let formats: [(&str, &str, Generator); 4] = [
        ("protobuf", "proto", protobuf),
        ("thrift", "thrift", thrift),
        ("smithy", "smithy", smithy),
        ("graphql", "graphql", graphql),
    ];
    println!(
        "{:>10} {:>10} {:>12} {:>10} {:>10}",
        "format", "operations", "bytes", "findings", "time"
    );
    for (name, extension, generate) in formats {
        for &n in sizes {
            let dir = tempfile::tempdir().expect("temporary directory");
            // Each version in its own directory: Smithy reads the files
            // beside the entry.
            std::fs::create_dir_all(dir.path().join("old")).unwrap();
            std::fs::create_dir_all(dir.path().join("new")).unwrap();
            let old = format!("old/bench.{extension}");
            let new = format!("new/bench.{extension}");
            std::fs::write(dir.path().join(&old), generate(n, false)).unwrap();
            let text = generate(n, true);
            std::fs::write(dir.path().join(&new), &text).unwrap();
            let target = Target::new(
                "bench",
                Source::new(dir.path(), &old),
                Source::new(dir.path(), &new),
            );
            let engine = Engine::default();
            let mut best = Duration::MAX;
            let mut findings = 0;
            for _ in 0..3 {
                let start = Instant::now();
                let comparison = engine.compare(std::slice::from_ref(&target));
                best = best.min(start.elapsed());
                assert!(
                    !comparison.diagnostics.has_errors(),
                    "{name}: {:?}",
                    comparison.diagnostics
                );
                findings = comparison.findings.len();
            }
            println!(
                "{name:>10} {n:>10} {:>12} {findings:>10} {:>8.0?}",
                text.len(),
                best
            );
        }
    }
}
