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
request comments), `github` (workflow annotations), `junit`, `gitlab`
(GitLab Code Quality, shown in merge requests), or `changelog`. The last is release notes in Markdown, grouped into breaking
changes, deprecations, additions and other changes.

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

## JSON Type Definition

Files ending in `.jtd.json` are read as JSON Type Definitions (RFC 8927),
used in the role given with `--role`:

```sh
faultline diff v1/event.jtd.json v2/event.jtd.json --role output
```

JTD objects are closed, so consumers that validate data reject properties
they do not know, and new properties in output are reported as risky. See
[examples/jtd](../examples/jtd) and the
[JSON Type Definition internals](internals/jtd.md).

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

## Smithy

Files ending in `.smithy` are read as Smithy IDL 2.0, together with the
`.smithy` files beside them. Operations with `@http` are compared as HTTP
operations bound by their traits, others as remote procedure calls:

```sh
faultline diff v1/model/weather.smithy v2/model/weather.smithy
```

A model without services is compared as its structures and unions, used in
the role given with `--role`. See [examples/smithy](../examples/smithy) and
the [Smithy internals](internals/smithy.md).

## Thrift

Files ending in `.thrift` are read as Thrift IDL with the files they
include. Each function of each service is compared, and arguments and
results are compared field by field, by id:

```sh
faultline diff v1/pets.thrift v2/pets.thrift
```

Integer widths and kinds of collection are separate wire types in Thrift,
so changing `i32` to `i64`, or `list` to `set`, is reported as breaking.
A file without services is compared as its structs, used in the role
given with `--role`. See [examples/thrift](../examples/thrift) and the
[Thrift internals](internals/thrift.md).

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

## Avro

Schemas ending in `.avsc` are read as Avro. Choose the compatibility mode
with `--role` (or `role` in an `[[api]]` entry):

```sh
faultline diff v1/pet.avsc v2/pet.avsc --role input    # BACKWARD
faultline diff v1/pet.avsc v2/pet.avsc --role output   # FORWARD
faultline diff v1/pet.avsc v2/pet.avsc                 # FULL
```

Faultline follows Avro's schema resolution. Readers ignore unknown fields,
fill in defaults and match renamed fields through aliases, and the numeric
promotions Avro allows are compatible. See [examples/avro](../examples/avro)
and the [Avro internals](internals/avro.md).

Protocols (`.avpr`) and IDL files (`.avdl`) are compared as their messages,
each an operation whose request is its parameters and whose response is
the response type or a declared error. IDL imports are followed:

```sh
faultline diff v1/orders.avdl v2/orders.avdl
```

See [examples/avro-idl](../examples/avro-idl).

## XML Schema

Files ending in `.xsd` are read as XML Schema, following `include` and
`import`. Each global element is a schema root, used in the role given with
`--role`:

```sh
faultline diff v1/order.xsd v2/order.xsd --role input
```

XML documents are compared as objects: child elements are members,
attributes are `@`-prefixed members, repeated elements are arrays, and a
`choice` is a group of members of which one is present. See
[examples/xsd](../examples/xsd) and the [XML Schema internals](internals/xsd.md).

## WSDL

Files ending in `.wsdl` are read as WSDL 1.1 or 2.0, with the schemas they
embed or import. Each operation of each port type (or WSDL 2.0 interface) is
compared, with its SOAP bodies, actions, headers and faults:

```sh
faultline diff v1/pets.wsdl v2/pets.wsdl
```

See [examples/wsdl](../examples/wsdl) and the [WSDL internals](internals/wsdl.md).

## RAML

Files ending in `.raml` are read as RAML 1.0, with `!include`s expanded and
libraries loaded from `uses`. Resource types and traits are applied first,
so each operation is compared as if it had been written out in full:

```sh
faultline diff v1/pets.raml v2/pets.raml
```

A change to a trait or to a type in a library is reported at each operation
that uses it, pointing into the library. See [examples/raml](../examples/raml)
and the [RAML internals](internals/raml.md).

## AsyncAPI

Documents with an `asyncapi` field are read as AsyncAPI 2.x or 3.0. Messages
an application receives are compared like requests it accepts, and messages
it sends like responses its consumers read:

```sh
faultline diff v1/asyncapi.yaml v2/asyncapi.yaml
```

Operations are identified by action and channel address
(`op[receive payments]`, `webhook[sent]/op[send orders/{}/placed]`), so a
description migrated from 2.6 to 3.0 compares cleanly with its old self.
Payloads may be JSON Schema, OpenAPI schemas or Avro. See
[examples/asyncapi](../examples/asyncapi) and the
[AsyncAPI internals](internals/asyncapi.md).

## gRPC services and their OpenAPI descriptions

A gRPC service exposed over HTTP/JSON with `google.api.http` rules can be
checked against the OpenAPI description published for its JSON clients:

```sh
faultline diff openapi.yaml proto/pets.proto --new-format protobuf-http
```

`protobuf-http` reads the `.proto` file as the HTTP/JSON API its rules
expose, with messages in their proto3 JSON form. Any breaking finding is a
promise of the description the service does not keep. `--old-format` and
`--new-format` set each side's format separately whenever two versions are
written in different formats. See
[examples/grpc-gateway](../examples/grpc-gateway).

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
needs enough history to contain the merge base (`fetch-depth: 0`). In GitLab
merge request pipelines it defaults to `$CI_MERGE_REQUEST_DIFF_BASE_SHA`,
the commit the merge request's diff starts from (set `GIT_DEPTH: 0` so it
is fetched).

The repository is also a GitHub Action, which installs the release for the
runner (verified against its checksums) and runs faultline:

```yaml
- uses: actions/checkout@v4
  with:
    fetch-depth: 0
- uses: integral0909/faultline@vX.Y.Z
  with:
    args: check --output github   # the default
```

Its `exit-code` output holds faultline's exit code. See
[examples/ci/github-actions.yml](../examples/ci/github-actions.yml), and
[examples/ci/gitlab-ci.yml](../examples/ci/gitlab-ci.yml) for a GitLab job
whose findings appear in the merge request's Code Quality report.

`diff` can read either side from a revision too. The new path defaults to the
old one:

```sh
faultline diff --old-rev v1.4.0 api/openapi.yaml          # tag vs working tree
faultline diff --old-rev v1.4.0 --new-rev v2.0.0 api/openapi.yaml
```

Files are read from the revision's tree under the same rules as the working
tree: references cannot leave the workspace root. Symbolic links and
submodules inside a revision are not followed.

## Releases

Every summary suggests the version bump the changes need. To enforce it,
set `versioning = "semver"` in `[policy]`: a declared version that
increases too little then fails the check (see
[configuration.md](configuration.md#versioning)).

An operation whose path or name changed while everything else stayed the
same is reported as moved (FL2004), not as a removal and an unrelated
addition.

## Overlays

When clients see a description with an overlay applied (OpenAPI Overlay
1.0), compare it that way. Here the overlay removes the operations marked
`x-internal`:

```sh
faultline diff v1/openapi.yaml v2/openapi.yaml --overlay public.yaml
```

For `check`, list the overlays of an API in `faultline.toml`
(`overlays = ["public.yaml"]`). Each version is read with its own copy of
the overlays. See [examples/overlay](../examples/overlay) and
[configuration.md](configuration.md#overlays).

## Taking usage into account

A usage index built from access logs or client queries softens findings
about parts of the API nobody uses (see
[configuration.md](configuration.md#usage)):

```sh
faultline usage import api/openapi.yaml --access-log access.log
```

## Adopting on an existing API

Record the current findings as accepted, commit the baseline, and only new
findings fail from then on:

```sh
faultline baseline v1/openapi.yaml v2/openapi.yaml   # writes .faultline/baseline.json
faultline diff v1/openapi.yaml v2/openapi.yaml --baseline .faultline/baseline.json
```

To accept one specific break temporarily, add a waiver with an expiry date to
`faultline.toml` (see [configuration.md](configuration.md#waivers)).

## In the editor

`faultline lsp` is a language server: editors that speak the Language Server
Protocol show findings while an API description is edited, before anything is
committed. It compares every `[[api]]` of `faultline.toml` (or, without one,
each open OpenAPI, AsyncAPI, JSON Schema, Protobuf, GraphQL or Avro document)
with the
last commit; pass `--base origin/main`, or set the `base` initialization
option, to compare with another revision. Hovering a finding explains it, and
its quick fix adds a 30-day waiver to `faultline.toml`.

For Neovim:

```lua
vim.lsp.config("faultline", {
  cmd = { "faultline", "lsp" },
  filetypes = { "yaml", "json", "proto", "graphql" },
  root_markers = { "faultline.toml", ".git" },
  init_options = { base = "origin/main" },
})
vim.lsp.enable("faultline")
```

See [internals/lsp.md](internals/lsp.md) for how the server works.

## With a schema registry

Teams that publish API descriptions to a central place can run
`faultline registry serve`, which stores each version and refuses the ones
that break earlier versions' clients. `faultline registry push` sends a
version from CI; `faultline registry check` asks whether one would be
accepted. See [registry.md](registry.md).

## Inspecting Faultline's model

`faultline ir dump openapi.yaml` prints the normalized model Faultline
compares, with the direction of every type. It is useful to understand why a
finding appears where it does.
