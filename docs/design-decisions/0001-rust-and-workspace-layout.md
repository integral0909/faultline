# 0001. Rust, and a workspace of focused crates

* **Status:** Accepted
* **Date:** 2024-02-11

## Context

Faultline parses large, untrusted documents in several formats, builds and
compares cyclic type graphs, and must run as a dependency-free binary in CI
pipelines on Linux, macOS and Windows. Its output must be deterministic. It also
needs to be embeddable: the language server and the registry server reuse the
same engine as the CLI.

## Alternatives

* **Go.** Simple static binaries and fast compilation. Weaker at modelling the
  IR: no sum types, so exhaustive handling of change kinds would rely on
  discipline and linters rather than the compiler.
* **TypeScript/Node.** Closest to the OpenAPI and GraphQL ecosystems. Needs a
  runtime in CI, is harder to make fast on multi-megabyte documents, and offers
  limited control over memory.
* **Rust.** Sum types and exhaustive matching, predictable performance, static
  binaries, a strong parsing and automata ecosystem (`regex-automata`,
  `gitoxide`), and `#![forbid(unsafe_code)]` for memory safety on hostile input.
  Slower compile times and a steeper learning curve for contributors.

For code organisation, the options were a single crate with modules, or a
workspace of crates.

## Decision

Faultline is written in Rust, edition 2024, and organised as a Cargo workspace
under `crates/`. Crate boundaries follow dependency direction: source handling
at the bottom, the IR above it, format front ends and the comparison engine
above that, and the facade, CLI, language server and registry at the top.
Versions, lints and metadata are defined once in the workspace manifest.

## Reasoning

The comparison engine is built around closed sets of type kinds and change
kinds; exhaustive `match` turns "every change is classified" into a
compile-time guarantee. Rust's performance lets the targets in the architecture
document be met without a separate native component.

Separate crates make the layering enforceable — a front end cannot reach into
the policy layer because it does not depend on it — let independent front ends
compile in parallel, and give the language server and registry a narrow API to
depend on.

## Trade-offs

* Compile times are higher than for Go; crate splitting partly offsets this.
* More `Cargo.toml` files to maintain; workspace inheritance keeps them small.
* Contributors from the API-tooling ecosystem are more often fluent in
  TypeScript than Rust.

## Consequences

* `unsafe` is forbidden workspace-wide; a genuine need requires a new ADR.
* The minimum supported Rust version is declared in `rust-version` and checked
  in CI. It is raised deliberately, not incidentally.
* New crates are introduced when their milestone begins, not ahead of time.
