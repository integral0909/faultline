//! Times the whole pipeline — parse, lower, compare, classify — on
//! generated OpenAPI descriptions of increasing size.
//!
//! Run with `cargo bench -p faultline`. The descriptions are generated
//! deterministically: `n` operations pairs over `n` object schemas that
//! reference each other in a chain, with a fixed share changed in the new
//! version (tightened bounds, added enum values, removed operations).

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use faultline::{Engine, Source, Target};

/// One version of a description with `n` resources.
fn description(n: usize, new: bool) -> String {
    let mut paths = String::new();
    let mut schemas = String::new();
    for i in 0..n {
        let max_name = if new && i % 50 == 0 { 32 } else { 64 };
        let extra = if new && i % 40 == 0 { ", d" } else { "" };
        let _ = write!(
            schemas,
            r##"    Item{i}:
      type: object
      required: [id, name]
      properties:
        id: {{type: string, pattern: "^[a-z0-9-]{{8,36}}$"}}
        name: {{type: string, maxLength: {max_name}}}
        count: {{type: integer, minimum: 0, maximum: 1000}}
        status: {{type: string, enum: [a, b, c{extra}]}}
        child: {{$ref: "#/components/schemas/Item{next}"}}
"##,
            next = (i + 1) % n
        );
        let put = if new && i % 100 == 0 {
            String::new()
        } else {
            format!(
                "    put:\n      requestBody: {{required: true, content: {{application/json: {{schema: {{$ref: \"#/components/schemas/Item{i}\"}}}}}}}}\n      responses: {{'204': {{description: ok}}}}\n"
            )
        };
        let _ = write!(
            paths,
            "  /items{i}/{{id}}:\n    parameters: [{{name: id, in: path, required: true, schema: {{type: string}}}}]\n    get:\n      parameters: [{{name: limit, in: query, schema: {{type: integer, maximum: 100}}}}]\n      responses: {{'200': {{description: ok, content: {{application/json: {{schema: {{$ref: \"#/components/schemas/Item{i}\"}}}}}}}}}}\n{put}"
        );
    }
    format!(
        "openapi: 3.1.0\ninfo: {{title: bench, version: 1.0.0}}\npaths:\n{paths}components:\n  schemas:\n{schemas}"
    )
}

fn main() {
    // `cargo test --benches` runs this with `--bench`-less arguments; keep
    // that quick.
    let quick = !std::env::args().any(|a| a == "--bench");
    let sizes: &[usize] = if quick { &[50] } else { &[250, 1000, 4000] };
    println!(
        "{:>10} {:>12} {:>10} {:>10}",
        "operations", "bytes", "findings", "time"
    );
    for &n in sizes {
        let dir = tempfile::tempdir().expect("temporary directory");
        std::fs::write(dir.path().join("old.yaml"), description(n, false)).unwrap();
        let text = description(n, true);
        std::fs::write(dir.path().join("new.yaml"), &text).unwrap();
        let target = Target::new(
            "bench",
            Source::new(dir.path(), "old.yaml"),
            Source::new(dir.path(), "new.yaml"),
        );
        let engine = Engine::default();
        // Best of three, to discount warm-up.
        let mut best = Duration::MAX;
        let mut findings = 0;
        for _ in 0..3 {
            let start = Instant::now();
            let comparison = engine.compare(std::slice::from_ref(&target));
            best = best.min(start.elapsed());
            assert!(!comparison.diagnostics.has_errors());
            findings = comparison.findings.len();
        }
        println!(
            "{:>10} {:>12} {findings:>10} {:>9.0?}",
            n * 2,
            text.len(),
            best
        );
    }
}
