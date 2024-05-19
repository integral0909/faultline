# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `faultline-source`: source map, spans, line index with byte/character/UTF-16
  columns, structured diagnostics with a plain-text renderer, and input
  resource limits.
- `faultline-doc`: spanned JSON reader and YAML 1.2 loader producing one
  document tree, with exact decimal numbers, duplicate-key detection, YAML
  merge keys, and depth, node-count and alias-expansion limits; RFC 6901 JSON
  Pointer and RFC 3986 URI reference resolution and normalization.
- Fuzz targets for the JSON reader, YAML loader and URI/pointer handling.
- `faultline-ir`: the format-independent contract model — type arena with
  reserve/define for recursive types, HTTP operation and schema-root surfaces,
  computed request/response polarity (callbacks, `not`, `readOnly`/
  `writeOnly`), stable textual `IrPath` identities with a parser, structural
  validation including unguarded-cycle detection, and a deterministic dump.
- `faultline-doc`: sandboxed document loading — `FsLoader` confined to the
  workspace root (including through symbolic links), strict URI resolution
  that reports `..` escaping the root, a caching `DocumentStore`, and remote
  documents refused unless explicitly enabled.
- `faultline-jsonschema`: JSON Schema front end for draft-04 through 2020-12 —
  identifier and anchor scanning, cross-document references, `$dynamicRef`
  and `$recursiveRef` resolved against the dynamic scope, lowering of every
  assertion and applicator keyword into the IR, and diagnostics with source
  spans for invalid schemas and unresolvable references.
- `faultline-subsume`: value-set inclusion between types of two APIs, with an
  exact validator, a boundary-seeking sampler, exact rational arithmetic, and
  a checker that answers yes, no with a verified counterexample, or unknown,
  handling recursive types coinductively.
- `faultline-diff`: comparison of two APIs — operations, parameters, bodies,
  responses, callbacks, schema roots and every type position — producing typed
  changes with polarity and evidence of lost and gained values.
- `faultline-openapi`: OpenAPI 3.0 and 3.1 front end — operations,
  parameters with effective serialization, request bodies, responses,
  headers, callbacks, webhooks, security requirements, discriminators, and
  every component schema, with references across files and FLE2xxx
  diagnostics.
- Diff support for parameter serialization, security alternatives, webhooks
  and discriminator mappings.
- `faultline-rules`: the rule catalog (FL1xxx value rules, FL2xxx HTTP rules)
  with explanations, classification of every change into breaking, risky or
  info according to who sends and who receives at its position, stable
  finding identifiers, and semantic-version suggestions.
- `faultline-policy`: `faultline.toml` with strict validation, severity
  overrides by rule, API and path glob, waivers with mandatory expiry, and
  baselines.
- `faultline-report`: text reports with source excerpts and verified example
  values, versioned JSON, SARIF 2.1.0, Markdown, GitHub annotations and JUnit.
- `faultline` and the `faultline` command: `diff`, `baseline`, `explain`,
  `rules` and `ir dump`, with documented exit codes.
- `faultline-vcs`: reading workspaces from git revisions without a checkout,
  with revision parsing, merge bases, the workspace sandbox applied to the
  revision's tree, size checks before reading blobs, and symbolic links and
  submodules refused.
- `faultline check`: compares every `[[api]]` in `faultline.toml` with the
  merge base of `--base` (or the base itself with `--no-merge-base`), from the
  working tree or `--head`; APIs missing from the base are reported as new.
- `diff` and `baseline` accept `--old-rev` and `--new-rev`, and the new path
  defaults to the old one.
- JSON report locations include the `revision` they were read from; SARIF and
  GitHub annotations prefer working-tree locations.
- IR support for RPC formats: RPC operations with streaming, exclusive field
  groups (`oneof`) understood by validation, sampling and inclusion checking,
  fields matched by wire tag, aliases, numbered enum values and wire
  encodings, with rules FL1025–FL1031 and FL3001.
- `faultline-protobuf`: proto2, proto3 and editions — lexer, parser with
  error recovery, import loading with built-in well-known types, `protoc`
  name resolution and validation (FLE3xxx), and lowering of the binary wire
  format. `.proto` entries are detected by extension.
- A fuzz target for the Protobuf front end, and a gRPC example.
- `faultline-net` and `[network]`: opt-in `https` references restricted to
  allowlisted hosts, with redirects checked hop by hop, connections refused
  to non-public addresses after resolution, no proxies, and size, time and
  redirect limits. Remote documents are fetched once per run.
