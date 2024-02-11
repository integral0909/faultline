# Contributing

Thanks for your interest in Faultline.

## Before you start

For anything larger than a small fix, please open an issue first describing the
problem and the approach you have in mind. Changes to the IR, the rule catalog
or the report format affect compatibility guarantees and are easier to review
once the direction is agreed.

## Making a change

1. Set up the toolchain as described in [docs/development.md](docs/development.md).
2. Keep each pull request focused on one logical change.
3. Add tests that fail without your change. New rules need corpus cases for
   both the breaking and the non-breaking direction.
4. Run the CI checks locally:

   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace --all-features
   ```

5. Update documentation and `CHANGELOG.md` when behaviour visible to users
   changes.

## Design decisions

Significant decisions are recorded as ADRs in
[docs/design-decisions/](docs/design-decisions/). If your change reverses or
extends one, add a new ADR rather than editing the old one.

## License

By contributing you agree that your contributions are dual licensed under the
MIT and Apache-2.0 licenses, as described in the README.
