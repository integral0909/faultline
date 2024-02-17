# 0002. Own spanned JSON reader; YAML through an event parser

* **Status:** Accepted
* **Date:** 2024-02-17

## Context

Every finding and diagnostic must point at the exact key or value in the
user's file, so the document layer has to keep a span for every node and every
object key. Inputs are untrusted and must be bounded in depth, size and — for
YAML — alias expansion. Numbers used as constraints (`maximum`, `multipleOf`,
`enum`) must compare exactly: `0.1` and `0.10000000000000001` are different
bounds even though they are the same `f64`.

## Alternatives

**JSON**

* `serde_json` — fast and ubiquitous, but discards positions, and its
  `arbitrary_precision` feature still offers no exact comparison.
* A span-preserving JSON crate — the available options either expose spans
  only for some nodes or pull in large dependency trees.
* **Our own reader** — RFC 8259 is small; a recursive-descent reader with spans
  and good error messages is a few hundred lines and fully under our control.

**YAML**

* `serde_yaml` — deprecated, and has no spans.
* A full YAML library's DOM (`saphyr`, `yaml-rust2`) — spans are partial and
  anchor expansion happens before we can bound it.
* **The event API of `saphyr-parser`** — a YAML 1.2 compliant tokenizer/parser
  that reports every event with a position, leaving tree construction,
  scalar typing and alias expansion to us.
* Writing a YAML parser — YAML's grammar is large and subtle; not justified.

## Decision

* JSON is read by `faultline-doc`'s own reader.
* YAML is read by consuming `saphyr-parser` events and building the tree in
  `faultline-doc`.
* Both produce the same `Node` tree with spans on every value and key.
* Numbers are stored as exact canonical decimals (`Decimal`), not `f64`.

## Reasoning

Owning tree construction lets every limit be enforced *before* allocation: an
alias is only copied after its node count has been charged against the
document limit, which makes "billion laughs" inputs fail fast instead of
exhausting memory. It also lets us make deliberate choices about YAML semantics
that matter for API descriptions:

* scalars use the YAML 1.2 core schema (`yes` is a string, `0o17` is an
  integer);
* mapping keys are always strings, so OpenAPI's `200:` response keys work;
* YAML 1.1 merge keys (`<<`) are honoured because hand-written specs use them;
* multiple documents in one file are an error rather than silently ignored.

The canonical decimal form (no leading or trailing zeros, normalized zero)
makes derived equality and hashing numeric, and ordering exact, without an
arbitrary-precision arithmetic dependency.

## Trade-offs

* We maintain a JSON reader; it is fuzzed continuously to compensate.
* The YAML parser reports positions in characters, not bytes, so we convert
  with a checkpoint table on non-ASCII input.
* The parser's own flow-nesting ceiling (255) can fire before our configured
  limit; we map it to the same diagnostic.
* Duplicate keys are errors. Some tools accept them silently; we consider an
  ambiguous contract unsafe to judge.

## Consequences

* Diagnostics from all JSON/YAML-based front ends share spans and codes
  (`FLE00xx`–`FLE02xx`).
* Consumers never see YAML-specific constructs; front ends are written against
  one JSON-shaped tree.
* The fuzz targets `parse_json`, `parse_yaml` and `uri_pointer` guard this
  layer; any change to it must keep them running clean.
