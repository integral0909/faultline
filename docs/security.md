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

**Git revisions.** Revisions are read from the object database in-process;
no `git` program, hook, filter or other configured command is run. The
workspace is the subtree of the revision under the workspace root, so the
containment rules above apply unchanged. Symbolic links in a revision are
refused rather than followed, submodules are not entered, and a blob's size
is checked against the file-size limit from its object header before it is
read.

**Network access and SSRF.** Remote references are disabled by default, and
the refusal is enforced by the document store for every loader, not by each
loader individually. When enabled in `[network]`, fetching is HTTPS-only and
restricted to an allowlist of hosts. Redirects are not followed by the HTTP
client: each hop is checked against the policy again, so a permitted host
cannot redirect to a forbidden one or downgrade to HTTP. Host names are
resolved inside the connection path and the connection is refused if any
resolved address is loopback, private, link-local, carrier-grade NAT,
documentation, multicast or otherwise reserved (IPv4-mapped, NAT64 and 6to4
IPv6 addresses are judged by the IPv4 address they carry). Because the
connection uses exactly the checked addresses, DNS rebinding cannot redirect
it. Proxies are not used, response size and time are capped, and bodies are
requested without compression so the size cap applies to what is read.

**Code execution.** Faultline never executes anything found in inputs: there
are no templates, scripts or plugins. Custom rules are declarative data.

**Secret handling.** Configuration refers to secrets indirectly (`env:VAR`) and
they are redacted from logs. Usage importers persist only templated paths,
field paths and counts — never query values, headers or bodies, which the
importers read past without storing.

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
