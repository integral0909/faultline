# Architecture

This document describes what Faultline is for, how it is structured, and the
order in which it is being built. Detailed rationale for individual decisions
lives in [design-decisions/](design-decisions/); algorithm-level notes live in
[internals/](internals/), starting with [the IR](internals/ir.md).

## Problem

API contracts change constantly and reviewers cannot reliably tell which
changes break consumers. Existing checkers each cover a single format, most
compare syntax rather than meaning — "required field added" is flagged the same
way whether it sits in a request (breaks callers) or a response (does not) —
and none take into account whether anybody actually uses the part that changed.

Faultline lowers every supported format into one typed intermediate
representation (IR) in which each position carries a **polarity** — whether
values flow into the API, out of it, or both — and decides compatibility by
reasoning about value sets. Proven breaks come with a concrete counterexample.

## Users and workflows

| Workflow | Entry point |
|---|---|
| CI gate: head vs merge base for every configured API | `faultline check --base origin/main` |
| Ad-hoc comparison of files or revisions | `faultline diff old.yaml new.yaml`, `faultline diff v1.4.0:api/ HEAD:api/` |
| Understand a finding | `faultline explain FL1023` |
| Release: semver suggestion and changelog | `faultline semver`, `faultline changelog --from v1.4.0` |
| Adopt on a legacy API | `faultline baseline write` |
| Accept a known break temporarily | waiver in `faultline.toml`, with mandatory expiry |
| Usage-aware gating | `faultline usage import ...`, consumed by `check` |
| Editor feedback | `faultline lsp` |
| Enforce compatibility on publish | `faultline registry serve` |
| Inspect the normalized model | `faultline ir dump` |

## Requirements

### Functional

* **Formats:** OpenAPI 3.0/3.1 (Swagger 2.0 by up-conversion), JSON Schema
  draft-07/2019-09/2020-12, Protobuf (proto2, proto3, Editions), GraphQL SDL,
  Avro, AsyncAPI 2.6/3.0.
* **Sources:** files, directories, git revisions (`rev:path`), stdin, and — only
  when explicitly allowed — HTTPS URLs.
* **Dimensions:** findings are classified separately for `wire` (bytes on the
  network), `source` (generated code) and `semantic` (documented behaviour).
* **Modes:** `backward`, `forward`, `full`, and transitive variants over a
  version history.
* **Findings** carry a stable id, rule code, severity
  (`breaking`/`risky`/`safe`/`info`), dimensions, IR path, spans in both
  versions, an explanation and, where possible, a witness value.
* **Policy:** per-rule and per-path severity overrides, waivers with reason,
  owner and expiry, baselines, and a threshold that drives the exit code.
* **Outputs:** human-readable text, JSON (own versioned schema), SARIF 2.1.0,
  Markdown, GitHub annotations, JUnit XML, changelog.

### Non-functional

* Byte-identical output for identical input, independent of thread count.
* Single static binary for Linux, macOS and Windows.
* Library crates usable on their own through the `faultline` facade.

### Performance targets

Measured on a 2023-class laptop, release build.

| Scenario | Target |
|---|---|
| Parse and lower a 5 MB OpenAPI document | < 300 ms |
| Full diff of two 10k-operation APIs | < 1 s, peak RSS < 500 MB |
| `check` over 1,000 schema files at two revisions | < 3 s cold, < 1 s with warm IR cache |
| LSP re-check after a single-file edit | < 150 ms |

### Non-goals

Style linting, code generation, mock servers, runtime traffic validation,
automatic fixing of breaking changes, a web UI in v1, and dynamically loaded
plugins in v1.

## Pipeline

```text
 base ─┐   SourceLoader (fs sandbox | git object db | https allowlist)
 head ─┤   Frontend: parse → resolve references/imports → validate → lower
       └─► Api IR (old), Api IR (new)
                 │
         Polarity analysis        (in / out / both for every position)
                 │
         Alignment                (keys → identities → scored rename candidates)
                 │
         Comparison engine ─────► Subsumption (yes / no + witness / unknown)
                 │
         ChangeSet                (typed change events)
                 │
         Rule classification      (exhaustive catalog → findings)
                 │
         Policy                   (overrides, waivers, baseline, usage index)
                 │
         Reporters                (text, JSON, SARIF, Markdown, annotations, ...)
```

## Crates

| Crate | Responsibility |
|---|---|
| `faultline-source` | Source files, spans, line index, source map, diagnostics, resource limits |
| `faultline-doc` | Spanned JSON/YAML document tree, JSON Pointer, URI reference resolution |
| `faultline-ir` | The IR: type arena, surfaces, identities (`IrPath`), provenance, polarity |
| `faultline-regex` | ECMA-262 pattern parsing, translation to automata, language inclusion |
| `faultline-subsume` | Constraint lattices, three-valued subsumption, witness synthesis |
| `faultline-jsonschema` | JSON Schema dialects, `$id`/`$anchor`/`$dynamicRef`, bundling, lowering |
| `faultline-openapi` | OpenAPI 3.0/3.1 (and 2.0 up-conversion) lowering |
| `faultline-protobuf` | Protobuf lexer, parser, linker, well-known types, lowering |
| `faultline-graphql` | GraphQL SDL lexer, parser, validation, lowering |
| `faultline-avro` | Avro schema parsing, name resolution, lowering |
| `faultline-asyncapi` | AsyncAPI 2.6/3.0 lowering |
| `faultline-xml` | Spanned, namespace-aware XML reader without DTD processing |
| `faultline-xsd` | XML Schema reading (includes, imports) and lowering |
| `faultline-wsdl` | WSDL 1.1 reading, SOAP bindings, lowering |
| `faultline-diff` | Alignment, rename detection, comparison engine, change model |
| `faultline-rules` | Rule catalog, classification, semver derivation |
| `faultline-policy` | `faultline.toml`, overrides, waivers, baselines, verdicts |
| `faultline-usage` | Usage index and importers |
| `faultline-vcs` | Reading git revisions without a checkout |
| `faultline-net` | Remote document retrieval under the network policy |
| `faultline-report` | Reporters and the versioned JSON report model |
| `faultline` | Engine orchestration and the stable library API |
| `faultline-cli` | The `faultline` binary |
| `faultline-lsp` | Language server |
| `faultline-registry` | Schema registry server |
| `faultline-testkit` | Test corpus runner, generators, snapshot helpers (dev only) |

Crates are added as their milestone begins; the table describes the target
shape. Dependencies point strictly downwards: front ends depend on the IR, never
on each other (except OpenAPI and AsyncAPI reusing the JSON Schema front end),
and nothing below the facade knows about configuration files or the CLI.

## The IR

An `Api` holds **surfaces** (HTTP operations, RPC methods, GraphQL root fields,
message channels, or bare schemas) and an arena of **type nodes**. Type kinds
cover the union of what the supported formats can express: scalars with
representation details, objects with open/closed semantics, arrays, maps,
tuples, unions (inclusive `anyOf`, exclusive `oneOf`, tagged), intersections,
negation, enums with optional numeric tags, and named references, which is
where cycles live.

Each front end declares how fields are identified — by wire tag (Protobuf), by
name (JSON), or by name plus aliases (Avro) — and alignment uses that identity.

**Polarity** is computed rather than declared. Reachability is propagated from
each surface: request bodies, parameters, GraphQL arguments and consumed
messages are *in*; responses and produced messages are *out*; OpenAPI callbacks
invert direction; `readOnly`/`writeOnly` restrict it; a type reachable both ways
is *both*. For an *in* position the new version must accept a superset of
values; for *out* it must produce a subset; *both* requires equivalence.

**`IrPath`** is the stable, human-readable identity used in reports, waivers
and usage data, for example
`op[GET /pets/{}]/response[200]/body[application/json]/.items[]/.name`. Path
parameter names are normalized and spans are never part of it, so identities
survive reformatting and file moves.

## Comparison and subsumption

`compare(old, new, polarity)` is memoized on its arguments. A pair revisited
while still under comparison is assumed compatible (coinduction), which makes
recursive types terminate with the greatest-fixpoint answer.

Subsumption answers `yes`, `no(witness)` or `unknown(reason)`. `unknown` is
never treated as safe; policy decides whether it surfaces as `risky` (the
default) or is ignored. Cases the design specifically accounts for:

* adding a `oneOf` variant can *reject* values that now match two variants;
* `additionalProperties` narrowing interacts with `patternProperties`;
* `multipleOf` requires exact rational arithmetic;
* `pattern` inclusion is decided with automata under a state budget, falling
  back to `unknown` for look-around and backreferences.

Every witness is validated against both schemas before it is reported.

## Rules and policy

Change kinds form a closed enum and classification is a single exhaustive
match, so the compiler rejects an unclassified change. Rule codes are grouped by
area: `FL1xxx` structural, `FL2xxx` HTTP, `FL3xxx` Protobuf, `FL4xxx` GraphQL,
`FL5xxx` Avro, `FL6xxx` AsyncAPI. Each rule carries default severities per
polarity and dimension plus embedded documentation used by `explain`.

Configuration precedence is: built-in defaults < `faultline.toml` <
`FAULTLINE_*` environment variables < command-line flags. Unknown configuration
keys are errors.

## Errors and exit codes

Library crates expose typed errors and do not panic on user input. Input
problems are *diagnostics* with `FLE`-prefixed codes and spans, kept distinct
from compatibility *findings*.

| Exit code | Meaning |
|---|---|
| 0 | Passed |
| 1 | Findings exceed the policy threshold |
| 2 | Invalid input or configuration |
| 3 | Internal error |
| 4 | Expired waiver or stale baseline |

## Concurrency

The core is synchronous and data-parallel: base and head load concurrently,
files within an API parse in parallel, and APIs are compared in parallel.
Results are sorted before output. Async code exists only at the edges — the
registry server and optional HTTP fetching.

## Caching

Lowered IR is cached by a content hash of the normalized inputs, front-end
version and options. Cache entries carry a header (magic, format version, tool
version); any mismatch is a miss, never an error, and writes are atomic.

## Security boundaries

| Boundary | Enforcement |
|---|---|
| Untrusted bytes → document tree | size, depth, node-count and alias-expansion limits |
| Document → references | root sandbox, no network unless allowed, HTTPS + host allowlist + private-address refusal |
| References → IR | reference-chain and node-count limits |
| IR → subsumption | automaton state budget, recursion guard |
| Registry requests → engine | bearer token for writes, header/body size and time limits, a fixed pool of workers, lowering from the request's files only |

See [security.md](security.md) for the threat model.

## Roadmap

The milestones the project was built in, in order. The
[changelog](../CHANGELOG.md) records what each delivered.

| Milestone | Scope |
|---|---|
| M0 Foundation | Workspace, CI, licensing, documentation skeleton, `faultline-source` |
| M1 Documents | Spanned JSON parser, YAML loader, JSON Pointer, URI resolution |
| M2 IR core | Type arena, surfaces, `IrPath`, provenance, polarity, dump format |
| M3 JSON Schema | Dialects, reference resolution, bundling, lowering |
| M4 Diff engine | Alignment, coinductive comparison, subsumption v1, witnesses |
| M5 OpenAPI | 3.0/3.1 lowering and HTTP rules |
| M6 v0.1 | Rule catalog, policy, reporters, CLI (`diff`, `check`, `explain`, `baseline`) |
| M7 Git and workspaces | Revision loading, multi-API workspaces, remote reference policy |
| M8 Protobuf | Parser, linker, wire/JSON/source compatibility rules |
| M9 GraphQL | Parser, validation, variance-aware rules |
| M10 Avro | Schema resolution rules, compatibility modes, transitive history |
| M11 Advanced subsumption | Regex inclusion, formats, pattern properties, conditionals |
| M12 Renames and releases | Rename detection, semver, changelog |
| M13 Usage | Usage index and importers, policy integration |
| M14 Performance | Linear-time comparison, parallel pipeline, benchmarks |
| M15 LSP | Language server |
| M16 Registry | Registry server with compatibility enforcement |
| M17 AsyncAPI and cross-format | AsyncAPI, Protobuf ↔ OpenAPI consistency |
| M18 Hardening | Continuous fuzzing, security review, release pipeline |
