# 0004. Sandboxed, offline-by-default reference resolution

* **Status:** Accepted
* **Date:** 2024-03-08

## Context

Schemas reference other documents by URI: `$ref: ../common/pet.yaml`,
`$ref: https://example.com/schemas/address.json`. Faultline typically runs in
CI on schemas from pull requests, so a reference is attacker-controlled input
that asks Faultline to read something. Unrestricted, that means reading
arbitrary files on the CI runner (`../../../../etc/passwd`, or a symlink
committed into the repository) and making network requests to arbitrary
hosts, including internal ones.

RFC 3986 resolution silently discards `..` segments that climb above the root
of a path, so escape attempts do not even look like errors if resolution is
implemented to the letter.

## Alternatives

* **Trust the working directory.** Resolve relative references against real
  filesystem paths and read whatever they name. Simple, and unsafe on any
  shared runner.
* **Validate paths as strings.** Reject references containing `..`. Blocks
  legitimate sibling references (`../common.yaml`) and misses symlinks.
* **Workspace URIs with strict resolution and canonical containment.**
  Address files as `file:` URIs whose root is the workspace directory,
  detect `..` climbing above that root during resolution, and verify the
  canonical (symlink-resolved) path is inside the workspace before reading.

For network access: allow by default (convenient), allow with a warning, or
refuse by default and enable per host.

## Decision

* Workspace files are addressed as `file:///path/relative/to/root`. Local
  references are resolved with `UriRef::resolve_strict`, which reports a `..`
  that would climb above the root instead of clamping it.
* `FsLoader` canonicalizes the target and refuses anything whose canonical path
  is outside the canonical workspace root, then enforces the file-size limit
  while reading.
* `DocumentStore` refuses `http`/`https` documents unless remote access has
  been explicitly enabled, independent of which loader is in use.

## Reasoning

Using URIs for workspace files keeps JSON Schema's identifier semantics intact:
`$id` values and relative references compose by the RFC, and the same code
path handles embedded resources, other files and (when enabled) remote ones.
Detecting escapes at resolution time produces a precise diagnostic at the
offending `$ref`; canonical containment at load time closes the symlink gap
that no string check can.

Placing the network policy in the store rather than in each loader means a
future loader (git object database, test fixtures) cannot accidentally become a
network client.

## Trade-offs

* A workspace that legitimately references files outside its root must widen
  the root; there is no per-reference escape hatch.
* Schemas that reference public meta-schemas or shared registries over HTTPS
  need configuration before they can be checked.
* Canonicalization costs a system call per loaded document; documents are
  cached, so this is paid once per file.

## Consequences

* Diagnostics distinguish "outside the workspace" (`FLE1004`), "remote
  references disabled" (`FLE1005`) and ordinary load failures (`FLE1006`).
* Enabling remote references (a later milestone) must add host allowlists,
  private-address refusal after DNS resolution, and response-size limits
  before `allow_remote` is exposed in configuration.
