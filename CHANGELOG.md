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
