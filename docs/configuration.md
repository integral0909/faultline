# Configuration

Faultline reads `faultline.toml` from the workspace root (the current
directory unless `--root` is given), or the file passed with `--config`.
Every key is optional. Unknown keys are errors: a misspelled key in a CI gate
must not silently weaken it.

```toml
[workspace]
# Directory references may reach; nothing outside it is read.
root = "."

[[api]]
name = "public"                 # used in reports and finding ids
entry = "api/openapi.yaml"
# format = "openapi"            # or "asyncapi", "jsonschema", "protobuf", "protobuf-http", "graphql", "avro"; detected when omitted
# role = "input"                # schemas without operations: input | output | both

[policy]
fail_on = "breaking"            # breaking | risky | never
versioning = "off"              # off | semver: check declared versions
baseline = ".faultline/baseline.json"

[[policy.override]]
rule = "response-added"         # code (FL2040), name, or "*"
paths = ["op[* /internal/**]**"]
# api = "public"
severity = "info"               # breaking | risky | info

[[waiver]]
finding = "6059f23290bf"
reason = "Limit lowered deliberately; all known clients page at 50"
owner = "api-platform"
expires = "2026-12-31"

[usage]
# index = ".faultline/usage.json" # from `faultline usage import`
# unused_severity = "risky"     # cap for changes nobody uses

[network]
allow_remote = false            # fetch https references at all
allowed_hosts = []              # e.g. ["schemas.example.com", "*.example.net"]
# max_bytes = 8388608           # largest remote document
# timeout_seconds = 20          # per request, 1-300
# max_redirects = 5             # per document, at most 20
```

Command-line flags take precedence over the file: `--fail-on`, `--baseline`,
`--root`.

## Versioning

With `versioning = "semver"`, each API's declared version (OpenAPI
`info.version`) must increase by at least the bump its changes need:
* major for breaking changes;
* minor for risky changes and additions;
* patch for anything else.

Below `1.0.0`, a minor increase may break and a patch increase may add, as
Semantic Versioning allows. A version that increases too little fails the
check (exit code 1), even if no finding does:

```text
error: petstore declares 1.0.0 -> 1.1.0, a minor release, but its changes need a major release
```

APIs whose versions are not both semantic versions (`2024-06`, a format
without a version) are not checked.

## Overrides

Overrides change the severity of matching findings. A finding matches when
the rule matches (`"*"` matches every rule), the API matches (if given), and
any of `paths` matches its path (or `paths` is empty). When several overrides
match, the last one wins, so specific overrides go after general ones.

Paths are matched as globs over the finding's path:

| Pattern | Matches |
|---|---|
| `*` | any characters except `/` |
| `**` | any characters, including `/` |
| `?` | one character except `/` |

`op[* /internal/**]**` matches every finding in every operation whose path
starts with `/internal/`.

## Waivers

A waiver accepts one finding, identified by its finding id, until a date.
Waivers must have a reason and an expiry: accepting a break is a decision for
now, not forever. On the expiry date the waiver still applies; from the next
day it no longer does, the finding counts again, and the stale waiver itself
fails the check (exit code 4) until it is renewed or removed. Waivers that no
longer match any finding are reported so they can be deleted.

Finding ids depend on the API name, rule, path and a detail such as the enum
value — not on line numbers — so reformatting or moving files keeps them
stable. Renaming the API changes every id.

## Baselines

`faultline baseline OLD NEW` writes every current finding to
`.faultline/baseline.json` (or `--out`). Findings in the baseline do not fail
checks. The file is JSON, sorted for reviewable diffs:

```json
{
  "version": 1,
  "findings": [
    { "id": "978caf35da0c", "api": "openapi", "rule": "FL2001", "path": "op[DELETE /pets/{}]" }
  ]
}
```

Only `id` is matched; the other fields are for readers.

## Usage

A usage index records which operations, query parameters and (from GraphQL
client documents) fields and arguments clients use. With `[usage] index`
set, a finding about a part of the API the index shows unused is capped
at `unused_severity` (default `risky`) and marked as unused. For example,
an operation removed while nobody calls it. Parts the index does not
cover are unaffected.

Build an index from server access logs (Common or Combined Log Format, or
JSON lines with `method` and `path`), matched to the API's path templates:

```sh
faultline usage import api/openapi.yaml --access-log logs/*.log --base-path /v1 --window "last 30 days"
```

or from the GraphQL documents clients send:

```sh
faultline usage import schema.graphql --graphql clients/**/*.graphql
```

`--merge` adds to an existing index. Indexes hold operation identities,
parameter names, field paths and counts, never request values, headers,
bodies or client identities, so they can be committed.

## Remote references

References to `https://` documents are refused unless `[network]` enables
them, and then only for hosts on `allowed_hosts`: exact names, or
`*.domain` for every subdomain (not the domain itself). Faultline then:

* fetches over HTTPS only, and never sends credentials from the URL;
* follows redirects itself, one hop at a time, checking every target
  against the same rules;
* resolves the host and refuses to connect if any of its addresses is
  loopback, private, link-local or otherwise not publicly routable, so an
  allowlisted name cannot be pointed at internal services;
* ignores proxy settings, since the address check must apply to the host
  actually contacted;
* caps each document at `max_bytes` (and the file-size limit) and each
  request at `timeout_seconds`.

Each remote document is fetched once per run and shared by both versions
being compared. Refusals and failures are reported at the reference
(`FLE1005` and `FLE1006`).

## Environment

| Variable | Effect |
|---|---|
| `FAULTLINE_TODAY` | The date (`YYYY-MM-DD`) used for waiver expiry, for reproducible runs |

## JSON report

`--output json` produces a document with `schema_version: 1`. Fields may be
added within a version; incompatible changes increment it.

| Field | Content |
|---|---|
| `passed`, `exit_code`, `fail_on` | the verdict |
| `summary` | counts of active `breaking`, `risky` and `info` findings, `waived`, `baselined`, `failing` |
| `apis[]` | `name`, `old_version`, `new_version`, `suggested_bump` |
| `version_violations[]` | `api`, `old_version`, `new_version`, `declared_bump`, `required_bump` |
| `findings[]` | `id`, `api`, `rule`, `rule_name`, `dimension`, `severity`, `original_severity`, `failing`, `status`, `message`, `path`, `polarity`, `old_location`, `new_location`, `witness` |
| `expired_waivers[]`, `unused_waivers[]` | waivers needing attention |
| `diagnostics[]` | input problems: `severity`, `code`, `message`, `location` |

Locations are `{file, revision, line, column, end_line, end_column}` with one-based
lines and columns in characters; `revision` names the git revision the file
was read from, and is `null` for the working tree. `witness` is the example
value as JSON.
