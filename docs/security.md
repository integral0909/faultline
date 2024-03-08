# Security model

Faultline is routinely run in CI on schema files taken from pull requests, so it
treats every input document as potentially hostile. This page describes what
Faultline defends against and how. To report a vulnerability, see
[SECURITY.md](../SECURITY.md).

## Assets and trust

| Input | Trust level |
|---|---|
| Schema documents and everything they reference | untrusted |
| Usage data (HAR files, access logs, operation documents) | untrusted |
| `faultline.toml` | trusted as configuration, validated strictly |
| Command-line flags and `FAULTLINE_*` environment variables | trusted |
| Registry API requests | untrusted until authenticated, then authorized per subject |

## Threats and mitigations

**Resource exhaustion.** Every stage that can be made to do unbounded work has a
ceiling in `Limits`: file size (64 MiB), nesting depth (256), nodes per document
including YAML alias expansion (which defeats "billion laughs" documents),
reference-chain length, files per API, IR nodes, and automaton states for
regular-expression reasoning. Exceeding a limit is a diagnostic, not a crash.

**Filesystem escape.** Workspace files are addressed as `file:` URIs rooted
at the workspace directory. `..` segments that would climb above the root are
reported during resolution rather than silently clamped, and the loader
refuses any file whose canonical (symlink-resolved) path lies outside the
canonical workspace root.

**Network access and SSRF.** Remote references are disabled by default, and
the refusal is enforced by the document store for every loader, not by each
loader individually. When
enabled, fetching is HTTPS-only, restricted to an allowlist of hosts, refuses
addresses in private, loopback and link-local ranges after DNS resolution, and
caps response size and redirects.

**Code execution.** Faultline never executes anything found in inputs: there
are no templates, scripts or plugins. Custom rules are declarative data.

**Secret handling.** Configuration refers to secrets indirectly (`env:VAR`) and
they are redacted from logs. Usage importers persist only templated paths,
field paths, counts and timestamps — never query values, headers or bodies.

**Registry.** Bearer tokens are random, high-entropy and stored as SHA-256
hashes. Access is authorized per subject, request bodies are size-limited and
rate limits apply.

**Supply chain.** Dependencies come from crates.io only, the lockfile is
committed, and `cargo-deny` checks advisories, licenses and sources in CI.
`unsafe` code is forbidden across the workspace.

## Out of scope

Faultline does not protect against a malicious `faultline.toml` or malicious
command-line arguments supplied by the user running it, and it makes no
confidentiality guarantees about schema contents beyond not transmitting them
anywhere: it has no telemetry.
