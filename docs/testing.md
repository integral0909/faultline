# Testing

Faultline's correctness claims — "this change is breaking", "this one is
not" — are only as good as its tests. This page describes the kinds of tests
in the repository and when to add each.

## Running tests

```sh
cargo test --workspace                 # everything
cargo test -p faultline-doc            # one crate
cargo test -p faultline-doc --test yaml merge   # one test file, filtered by name
```

Property tests use [proptest](https://docs.rs/proptest). A failing case is
shrunk and printed; proptest also stores it under `proptest-regressions/` next
to the test file so it is replayed on every later run. Commit those files.

## Kinds of tests

| Kind | Where | Use for |
|---|---|---|
| Unit tests | `#[cfg(test)] mod tests` beside the code | behaviour of private functions and invariants |
| Integration tests | `crates/<crate>/tests/*.rs` | the crate's public API, as a user of the crate sees it |
| Property tests | either of the above, with `proptest!` | laws that must hold for all inputs: round trips, idempotence, agreement with a reference |
| Fuzz targets | `fuzz/fuzz_targets/` | untrusted-input code paths: never panic, stay within limits, keep invariants |

Guidelines:

* **Test behaviour, not implementation.** Assert on what a user could observe
  — values, spans, diagnostic codes — rather than on internal structure.
* **Pin error locations.** Parser tests assert the exact text under the
  primary label, not just that an error occurred.
* **Keep a reference where one exists.** Examples from specifications (RFC
  3986 §5.4, RFC 6901 §5) are used verbatim as test vectors.
* **Every fuzz finding becomes a regression test** in the relevant crate, with
  a comment saying it was found by fuzzing.

## Fuzzing

Fuzzing requires a nightly toolchain and `cargo-fuzz`:

```sh
rustup toolchain install nightly --profile minimal
cargo install --locked cargo-fuzz
cd fuzz
cargo +nightly fuzz run parse_json -- -max_total_time=120
```

Available targets:

| Target | Exercises |
|---|---|
| `parse_json` | JSON reader; round trip through the serializer |
| `parse_yaml` | YAML loader under tight node and depth limits |
| `uri_pointer` | URI resolution and normalization idempotence; JSON Pointer round trips |
| `lower_jsonschema` | JSON Schema lowering of an entry document and one referenced document; IR invariants |

Hand-written seed inputs live in `fuzz/seeds/<target>/` and are committed;
the generated corpus in `fuzz/corpus/` is not. Pass both directories to seed a
run:

```sh
cargo +nightly fuzz run lower_jsonschema corpus/lower_jsonschema seeds/lower_jsonschema
```

Crashes are written to `fuzz/artifacts/<target>/`. Reproduce one with
`cargo +nightly fuzz run <target> <file>`. CI fuzzes every target for one
minute on pull requests that touch the document layer and for ten minutes
nightly.
