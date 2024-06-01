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

## Protobuf and gRPC

`.proto` files are recognized by their extension:

```sh
faultline diff v1/pets/v1/pets.proto v2/pets/v1/pets.proto
```

Imports are resolved the way `protoc -I` would resolve them from the entry
file's directory or any directory above it within the workspace, and the
well-known types (`google/protobuf/timestamp.proto` and friends) are built
in. A file with services is compared as RPC operations
(`op[pets.v1.PetService/GetPet]`). A file without services is compared as
data, with each top-level message a schema root; set `role` in the `[[api]]`
entry when messages only flow one way.

Protobuf's binary format identifies fields and enum values by number, so
Faultline does too. Renumbering a field or an enum value, or changing its
wire encoding (`int32` to `sint32`), is breaking. Renaming one is risky: it
only breaks JSON encodings and generated code. See
[examples/grpc](../examples/grpc) and the
[Protobuf internals](internals/protobuf.md).

## GraphQL

Schema files ending in `.graphql`, `.graphqls` or `.gql` are read as GraphQL
SDL:

```sh
faultline diff v1/schema.graphql v2/schema.graphql
```

Each root field is an operation (`op[query/pets]`). Removing an output field,
making an output nullable, adding a required argument or input field, and
adding a union member break clients. Adding fields and optional arguments
does not. Arguments of nested fields appear in paths as
`….nicknames().locale`. See [examples/graphql](../examples/graphql) and the
[GraphQL internals](internals/graphql.md).

## Checking pull requests

List the APIs in `faultline.toml` at the root of the repository:

```toml
[[api]]
name = "petstore"
entry = "api/openapi.yaml"
```

Then compare the working tree with the branch it will merge into:

```sh
faultline check --base origin/main
```

`check` reads the old version of every API straight from git, without a
second checkout, and compares it with the working tree (or with a commit,
given `--head`). The old version is taken from the **merge base** of the base
and the head — the commit the branch started from — so changes merged into
`main` since then are not reported as if the branch had reverted them. Pass
`--no-merge-base` to compare with the base revision itself.

Locations in the old version carry the revision (`3f9a2c1b7d4e:api/openapi.yaml`).
An API that does not exist in the base yet is reported as new and not
compared. `--api NAME` checks only the named APIs. In GitHub Actions pull
request builds, `--base` defaults to `origin/$GITHUB_BASE_REF`; the checkout
needs enough history to contain the merge base (`fetch-depth: 0`). See
[examples/ci/github-actions.yml](../examples/ci/github-actions.yml).

`diff` can read either side from a revision too. The new path defaults to the
old one:

```sh
faultline diff --old-rev v1.4.0 api/openapi.yaml          # tag vs working tree
faultline diff --old-rev v1.4.0 --new-rev v2.0.0 api/openapi.yaml
```

Files are read from the revision's tree under the same rules as the working
tree: references cannot leave the workspace root. Symbolic links and
submodules inside a revision are not followed.

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
