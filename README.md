# Faultline

Faultline finds breaking changes in API contracts before your consumers do.

It reads OpenAPI, AsyncAPI, JSON Schema, Protobuf, GraphQL and Avro, lowers
each into a
single typed model in which every position knows whether data flows **into**
the API or **out of** it, and decides compatibility semantically: a change is
breaking when some value that used to be valid no longer is, in the direction
that matters. When Faultline can prove a break it shows a concrete example
value that demonstrates it.

```text
breaking[FL1007]: op[GET /pets]/param[query:limit]: maximum changed from 100 to 50
  --> v2/openapi.yaml:13:13
   |
13 |             type: integer
   |             ^^^^^^^^^^^^^ now (continues to line 15)
  --> v1/openapi.yaml:13:13
   |
13 |             type: integer
   |             ------------- before (continues to line 15)
   |
   = note: example: 100 was accepted and is now rejected
   = note: finding id: 6059f23290bf
   = help: `faultline explain FL1007` describes this rule
```

> **Status:** early development (0.1). OpenAPI 3.0/3.1, AsyncAPI 2.x/3.0,
> JSON Schema (draft-04 to 2020-12), Protobuf (proto2, proto3, editions),
> GraphQL SDL and Avro are supported, with text, JSON, SARIF, Markdown,
> GitHub annotation and JUnit output. Pull requests can be checked against
> their base branch straight from git, editors show findings through a
> language server, and a schema registry refuses breaking versions. See the
> [roadmap](docs/architecture.md#roadmap).

## Quick start

```sh
cargo install --locked --path crates/faultline-cli
faultline diff examples/petstore/v1/openapi.yaml examples/petstore/v2/openapi.yaml
```

See [getting started](docs/getting-started.md) and
[configuration](docs/configuration.md).

## Why another compatibility checker?

* **One tool across formats.** One configuration file, one policy model and
  one report format for REST, gRPC, GraphQL and event schemas.
* **Direction-aware.** Adding a required field to a request breaks callers;
  adding one to a response does not. Faultline computes the direction of every
  position — including callbacks, `readOnly`/`writeOnly` and GraphQL input
  types — instead of guessing from syntax.
* **Semantic, with evidence.** Constraint changes (`pattern`, `oneOf`,
  numeric bounds, `additionalProperties`, ...) are decided by checking value
  sets, and every proven break comes with a witness value that is validated
  against both schemas before it is shown.
* **Honest about uncertainty.** When a question cannot be decided (for
  example a regular expression with look-around), the result is *unknown*, never
  silently *safe*.
* **Built for CI.** Reads git revisions straight from the object database, has
  stable finding ids for waivers (which must expire), and emits SARIF, JSON,
  Markdown and GitHub annotations.
* **Beyond CI.** A language server shows findings in the editor before a
  change is committed, and a schema registry refuses breaking versions at
  publication time.
* **Safe on untrusted input.** Offline by default, sandboxed reference
  resolution, and resource limits on every parser.

## Building from source

Faultline is written in Rust. With [rustup](https://rustup.rs) installed:

```sh
git clone https://github.com/integral0909/faultline
cd faultline
cargo build --release
cargo test --workspace
```

See [docs/development.md](docs/development.md) for the full toolchain setup,
including the tools used by CI.

## Documentation

* [Getting started](docs/getting-started.md)
* [Configuration](docs/configuration.md)
* [Schema registry](docs/registry.md)
* [Troubleshooting](docs/troubleshooting.md)
* [Performance](docs/performance.md)
* [Rule reference](docs/rules.md)
* [Architecture](docs/architecture.md)
* [Development](docs/development.md)
* [Testing](docs/testing.md)
* [Design decisions](docs/design-decisions/)
* [Security policy](SECURITY.md)

## License

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
* MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
