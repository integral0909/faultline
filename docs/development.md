# Development

## Toolchain

| Tool | Version | Used for |
|---|---|---|
| Rust (stable) | latest stable, via `rust-toolchain.toml` | building, testing, linting |
| Rust (MSRV) | `rust-version` in `Cargo.toml` (currently 1.88) | CI compatibility check |
| Rust (nightly) | any recent nightly | fuzzing only (`cargo fuzz`) |
| [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) | 0.18+ | advisories, licenses, dependency sources |
| [tokei](https://github.com/XAMPPRocky/tokei) | 12+ | code size reports (optional) |

Install with [rustup](https://rustup.rs):

```sh
rustup toolchain install stable      # picked up automatically from rust-toolchain.toml
rustup toolchain install 1.88 --profile minimal
rustup toolchain install nightly --profile minimal   # only for fuzzing
cargo install --locked cargo-deny tokei
```

If Rust is also installed through a system package manager, make sure
`~/.cargo/bin` comes first on your `PATH` so that `rust-toolchain.toml` is
honoured.

## Everyday commands

These are exactly the checks CI runs:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo +1.88 check --workspace --all-features
cargo deny check advisories bans licenses sources
```

## Workspace layout

Each crate lives in `crates/<name>` and inherits version, edition, license and
lint configuration from the workspace `Cargo.toml`. New crates must add

```toml
[lints]
workspace = true
```

and declare dependencies through `[workspace.dependencies]` so versions stay in
one place.

## Conventions

* **No `unsafe`.** The workspace forbids it. If a future need arises it
  requires an ADR.
* **Errors.** Library crates expose typed error enums and never panic on user
  input. Panics are reserved for violated internal invariants.
* **Determinism.** Anything that reaches output must be ordered. Do not
  iterate a `HashMap` into a report; use `IndexMap`, `BTreeMap` or sort.
* **Comments** explain *why* — invariants, trade-offs, spec references — not
  what the next line does.
* **Tests** live next to the code for unit behaviour (`#[cfg(test)]`) and in a
  crate's `tests/` directory for public-API behaviour.

## Commit messages

Use the imperative mood and describe the change as a whole:

```text
Add spanned JSON parser with duplicate-key detection

Longer explanation of why, if it is not obvious from the diff.
```
