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
  encodings, with rules FL1025–FL1031 and FL3001; reserved numbers and names,
  with rules for unreserved removals, reuse and dropped reservations
  (FL1032–FL1034).
- `faultline-protobuf`: proto2, proto3 and editions — lexer, parser with
  error recovery, import loading with built-in well-known types, `protoc`
  name resolution and validation (FLE3xxx), and lowering of the binary wire
  format. `.proto` entries are detected by extension.
- A fuzz target for the Protobuf front end, and a gRPC example.
- Field arguments in the IR (`Field.arguments`, the `.field()` path
  segment), flowing opposite to their field.
- `faultline-graphql`: GraphQL SDL — lexer, parser with recovery, extension
  merging, type-system validation (FLE4xxx), and lowering with root fields
  as operations, `__typename` on every object and nullability as unions with
  `null`. `.graphql`, `.graphqls` and `.gql` entries are detected. A fuzz
  target and a GraphQL example.
- `faultline-avro`: Avro schemas — named types and namespaces, the checks
  Avro parsers apply (FLE5xxx, including union defaults), and lowering as
  readers see data, with BACKWARD, FORWARD and FULL compatibility as the
  `input`, `output` and `both` roles. `.avsc` entries are detected. A fuzz
  target and an Avro example.
- Writer and reader views: types marked `writer_exact` are validated and
  sampled as written on the producing side of every inclusion check and as
  read on the accepting side. Readers honour field aliases, renamed fields
  are paired through aliases, and rejections list every missing field.
- `faultline-regex`: ECMA-262 patterns as automata — a parser with the
  Annex B leniencies, anchors anywhere, subset construction over a
  partitioned alphabet with size and work limits, exact matching, samples,
  and shortest-difference search with length bounds. `pattern` is now
  validated exactly, sampled, and compared as a regular language, with
  witnesses. Property-tested against the `regex` crate and fuzzed.
- Performance: comparisons scale linearly with API size (8,000 operations in
  about 0.6 s); the two versions of an API, and the APIs of `faultline check`,
  are processed in parallel with deterministic output; comparison and
  inclusion recursion are depth-bounded; column lookups in minified
  documents are constant time. `cargo bench -p faultline` measures the
  pipeline on generated descriptions.
- `faultline-usage` and `faultline usage import`: usage indexes from access
  logs (Common/Combined Log Format or JSON lines, matched to path templates)
  and from GraphQL client documents (fields and arguments selected, with
  fragments expanded). With `[usage] index`, findings about unused parts of
  an API are capped at `unused_severity` and marked in reports. Indexes hold
  counts of templated operations, parameter names and field paths only.
- `faultline-lsp` and `faultline lsp`: a language server that compares the
  workspace's APIs (or, without configuration, each open API document) with
  a base revision as documents are edited, unsaved text included. Findings
  appear where the change is, with the rule and an example value on hover
  and a quick fix that adds a waiver to `faultline.toml`; input errors,
  expired waivers and too small version increases are shown too.
- `faultline-registry` and `faultline registry serve|push|check`: a schema
  registry that stores versions of API descriptions in any supported format
  and refuses versions whose findings fail the subject's settings (against
  the latest version, or every version when transitive). Its HTTP/1.1 server
  is bounded in header size, body size, time and concurrency, refuses
  chunked bodies, and requires a bearer token for writes when configured.
  Registered files can be fetched individually. A fuzz target covers request
  parsing and bundle validation.
- `faultline-asyncapi`: AsyncAPI 2.0–2.6 and 3.0 front end. Operations are
  lowered from the application's point of view (received messages flow in,
  sent messages out, 3.0 replies are responses) and identified by action and
  channel address, so migrations between specification versions compare
  cleanly. Messages are compared as their content type, headers and payload;
  payloads may be JSON Schema, OpenAPI schemas or Avro, which is lowered by
  the Avro front end into the same type graph. New rules FL2080
  (message-added) and FL2081 (message-removed); channels whose address
  changed are reported as moved (FL2004). Adds an example, a fuzz target and
  `format = "asyncapi"`.
- The IR gains message operations, `message[..]` path segments and grafting
  of one type graph into another; JSON Schema navigation shared by the
  OpenAPI and AsyncAPI front ends moved into `faultline-jsonschema`.
- Cross-format checks: `format = "protobuf-http"` (`--format`,
  `--old-format`, `--new-format`) reads a `.proto` file's services as the
  HTTP/JSON API their `google.api.http` rules expose, with path, query and
  body bindings, `additional_bindings`, `response_body`, proto3 JSON types
  and `(google.api.field_behavior) = REQUIRED`, so a gRPC service can be
  compared with its OpenAPI description. `google/api/annotations.proto`,
  `http.proto` and `field_behavior.proto` are built in, and imports used
  only by custom options are no longer reported as unused.
- Swagger 2.0: descriptions with `swagger: "2.0"` are lowered as the
  official converters translate them to OpenAPI 3 (body and `formData`
  parameters as request bodies, `consumes`/`produces` as media types,
  inline parameter schemas, `collectionFormat` as `style`/`explode`,
  `x-nullable`, `type: file`), so Swagger versions compare with each other
  and a migration to OpenAPI 3 compares cleanly. Other Swagger versions
  remain `FLE2010`.
- AsyncAPI message traits are applied (name, content type, schema format and
  headers, with 2.x and 3.0 precedence), as is a 2.x operation trait's
  `operationId`; the FLE6006 warning is no longer emitted.
- `faultline usage import --har`: usage from recorded traffic (HTTP
  Archives). Besides calls and query parameter names, the names of declared
  header parameters and of request body members (JSON and forms) are
  recorded, so findings about request fields no client sends are softened
  too. Values, other headers, cookies and responses are never stored.
- Release pipeline: pushing a `vX.Y.Z` tag checks the version and release
  notes, packages every crate, builds the binary for Linux (x86-64 glibc
  and musl, AArch64), macOS and Windows with build provenance attestations,
  publishes a GitHub release with checksums, and publishes the crates after
  approval. `scripts/release-notes.sh` extracts a version's notes;
  `docs/releasing.md` describes the process.
- Hardening: nightly fuzzing keeps its corpus between runs; the language
  server reports an internal error during analysis instead of exiting; the
  remote document cache survives a panicking thread; `faultline registry`
  clients refuse to send the token over plain HTTP except to the local
  machine.
- Releases: `[policy] versioning = "semver"` checks declared versions
  against the bump the changes need (with SemVer's rules below 1.0.0) and
  fails on too small an increase; `--output changelog` writes release notes;
  operations that changed only their path or name are reported as moved
  (FL2004).
- Added and removed fields carry a direction derived from whether the object
  allows undeclared members, which makes more findings provably compatible;
  removing a required member is attributed its witness.
- `faultline-net` and `[network]`: opt-in `https` references restricted to
  allowlisted hosts, with redirects checked hop by hop, connections refused
  to non-public addresses after resolution, no proxies, and size, time and
  redirect limits. Remote documents are fetched once per run.
