# Getting started

## Install

Faultline is a single binary. Build it from source with a Rust toolchain
(see [development.md](development.md)):

```sh
cargo install --locked --path crates/faultline-cli
```

## Compare two versions

Point `faultline diff` at the old and the new entry document. The format
(OpenAPI 3.0/3.1 or JSON Schema) is detected from the document.

```sh
faultline diff v1/openapi.yaml v2/openapi.yaml
```

Using the pet store example in this repository:

```text
breaking[FL1007]: op[GET /pets]/param[query:limit]: maximum changed from 100 to 50
  --> examples/petstore/v2/openapi.yaml:13:13
   |
13 |             type: integer
   |             ^^^^^^^^^^^^^ now (continues to line 15)
  ...
   = note: example: 100 was accepted and is now rejected
   = note: finding id: 6059f23290bf
   = help: `faultline explain FL1007` describes this rule
...
openapi 1.0.0 -> 2.0.0: 5 breaking, 2 risky, 2 compatible; suggested version bump: major
result: failed (5 findings at or above breaking)
```

Every finding has:

* a **severity** — `breaking`, `risky` or compatible (`info`);
* a **rule** (`FL1007`), explained by `faultline explain FL1007` and listed in
  the [rule reference](rules.md);
* a **path** naming the position in the API
  (`op[GET /pets]/param[query:limit]`);
* where it could be shown, an **example value** that the old version accepted
  and the new one rejects (or that clients may now receive and could not
  before), verified against both versions;
* a stable **finding id** for waivers and baselines.

The same change can mean different things in different places. Lowering a
`maxLength` breaks clients sending a request but is harmless in a response;
adding an enum value is harmless in a request but risky in a response.
Faultline works out which direction values flow at every position,
including callbacks and `readOnly`/`writeOnly` properties.

A type shared by several operations is compared once per direction and
reported at the first position where it is reached.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | No finding at or above the failure threshold |
| 1 | Findings at or above the threshold (default: `breaking`) |
| 2 | Invalid input or configuration |
| 3 | Internal error |
| 4 | An expired waiver is still configured |

`--fail-on risky` also fails on risky findings; `--fail-on never` only
reports.

## Output formats

`--output` selects `text` (default), `json`, `sarif`, `markdown` (for pull
request comments), `github` (workflow annotations) or `junit`.

The JSON format is versioned by its `schema_version` field; see
[configuration.md](configuration.md#json-report) for its fields.

## Standalone JSON Schemas

For a JSON Schema that is itself the contract, say which way documents flow:

```sh
faultline diff old/event.json new/event.json --role output   # you publish these
faultline diff old/config.json new/config.json --role input  # others write these
```

With `--role both` (the default) any change to the accepted values is
breaking.

## Adopting on an existing API

Record the current findings as accepted, commit the baseline, and only new
findings fail from then on:

```sh
faultline baseline v1/openapi.yaml v2/openapi.yaml   # writes .faultline/baseline.json
faultline diff v1/openapi.yaml v2/openapi.yaml --baseline .faultline/baseline.json
```

To accept one specific break temporarily, add a waiver with an expiry date to
`faultline.toml` (see [configuration.md](configuration.md#waivers)).

## Inspecting Faultline's model

`faultline ir dump openapi.yaml` prints the normalized model Faultline
compares, with the direction of every type. It is useful to understand why a
finding appears where it does.
