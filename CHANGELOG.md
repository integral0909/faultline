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
