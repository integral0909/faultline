# Releasing

Releases are cut from `main` by pushing a version tag. The
[release workflow](../.github/workflows/release.yml) does the rest.

## Steps

1. Choose the version by Semantic Versioning. Until 1.0, a minor increase
   may change behaviour (new findings, changed severities, changed output)
   and a patch increase only fixes bugs.
2. Set `version` in `[workspace.package]` and in every
   `[workspace.dependencies]` entry of the root `Cargo.toml`, and run
   `cargo check` so `Cargo.lock` follows.
3. In `CHANGELOG.md`, rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD`
   and add a new, empty `## [Unreleased]` above it. Check the notes with
   `scripts/release-notes.sh X.Y.Z`.
4. Commit (`Release X.Y.Z`), open a pull request, and merge it once CI
   passes.
5. Tag the merge commit and push the tag:

   ```sh
   git tag -a vX.Y.Z -m "Faultline X.Y.Z"
   git push origin vX.Y.Z
   ```

## What the workflow does

1. **Check the tag.** The tag must match the workspace version, and the
   changelog must have a section for it. The test suite runs, and every
   crate is packaged and verified with `cargo publish --dry-run`.
2. **Build.** The `faultline` binary is built for Linux (x86-64 glibc and
   musl, AArch64), macOS (x86-64, Apple silicon) and Windows (x86-64), each
   archived with the README, changelog and licenses and smoke-tested on its
   own runner. Each archive gets a signed build provenance attestation,
   which `gh attestation verify <archive> --repo integral0909/faultline`
   checks.
3. **Publish the GitHub release** with the archives, a `SHA256SUMS` file
   and the changelog section as its notes. Versions with a pre-release
   suffix (`1.0.0-rc.1`) are marked as pre-releases.
4. **Publish the container image** `ghcr.io/integral0909/faultline`, built
   from the `Dockerfile` for linux/amd64 and linux/arm64 and tagged with the
   version (and `latest`, except for pre-releases). The image is
   smoke-tested and gets a provenance attestation too.
5. **Publish to crates.io**, after a maintainer approves the `crates-io`
   environment, which holds the registry token. `cargo publish --workspace`
   publishes the crates in dependency order.

If a step fails after the tag was pushed, fix the problem on `main`, delete
the tag locally and remotely, and tag again. A version already published to
crates.io cannot be republished: release the fix as the next patch version.

## Security fixes

Security fixes follow [SECURITY.md](../SECURITY.md): prepare them in a
private security advisory, release them as a patch version, and publish the
advisory with the release.
