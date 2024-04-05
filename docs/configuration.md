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
# format = "openapi"            # or "jsonschema"; detected when omitted
# role = "input"                # JSON Schema only: input | output | both

[policy]
fail_on = "breaking"            # breaking | risky | never
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
```

Command-line flags take precedence over the file: `--fail-on`, `--baseline`,
`--root`.

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
| `findings[]` | `id`, `api`, `rule`, `rule_name`, `dimension`, `severity`, `original_severity`, `failing`, `status`, `message`, `path`, `polarity`, `old_location`, `new_location`, `witness` |
| `expired_waivers[]`, `unused_waivers[]` | waivers needing attention |
| `diagnostics[]` | input problems: `severity`, `code`, `message`, `location` |

Locations are `{file, line, column, end_line, end_column}` with one-based
lines and columns in characters. `witness` is the example value as JSON.
