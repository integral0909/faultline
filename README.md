# Faultline

Faultline finds breaking changes in API contracts before your consumers do.

It reads OpenAPI, JSON Schema, Protobuf, GraphQL and Avro, lowers each into a
single typed model in which every position knows whether data flows **into**
the API or **out of** it, and decides compatibility semantically: a change is
breaking when some value that used to be valid no longer is, in the direction
that matters. When Faultline can prove a break it shows a concrete example
value that demonstrates it.

```text
breaking[FL2013]: request body now rejects values it used to accept
  --> api/openapi.yaml:48:13
   |
48 |             maxLength: 32
   |             ^^^^^^^^^^^^^ was 64
   |
   = path: op[POST /pets]/request/body[application/json]/.name
   = example: {"name": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
```

> **Status:** early development. The foundation crates are in place; format
> front ends, the comparison engine and the CLI are being built milestone by
> milestone (see [docs/architecture.md](docs/architecture.md#roadmap)). The
> example above shows the intended output format.

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
