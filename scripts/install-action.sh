#!/usr/bin/env bash
# Installs faultline for the GitHub Action (action.yml): the release archive
# for this runner, verified against the release's SHA256SUMS, or, when there
# is none (or INSTALL=source), a build of the action's own source.
#
# Environment: VERSION (`latest` or X.Y.Z), INSTALL (`release` or `source`),
# RUNNER_OS_NAME, RUNNER_ARCH_NAME, GH_TOKEN, GITHUB_ACTION_PATH, GITHUB_PATH.
set -euo pipefail

repository="integral0909/faultline"
root="${RUNNER_TEMP:-/tmp}/faultline"
bin_dir="$root/bin"
mkdir -p "$bin_dir"

from_source() {
    echo "Building faultline from $GITHUB_ACTION_PATH"
    cargo install --locked --root "$root" --path "$GITHUB_ACTION_PATH/crates/faultline-cli"
    echo "$bin_dir" >> "$GITHUB_PATH"
}

if [ "${INSTALL:-release}" = "source" ]; then
    from_source
    exit 0
fi

case "$RUNNER_OS_NAME/$RUNNER_ARCH_NAME" in
    Linux/X64) target="x86_64-unknown-linux-musl" ;;
    Linux/ARM64) target="aarch64-unknown-linux-gnu" ;;
    macOS/X64) target="x86_64-apple-darwin" ;;
    macOS/ARM64) target="aarch64-apple-darwin" ;;
    Windows/X64) target="x86_64-pc-windows-msvc" ;;
    *)
        echo "No release archive for $RUNNER_OS_NAME/$RUNNER_ARCH_NAME"
        from_source
        exit 0
        ;;
esac

tag=""
if [ "${VERSION:-latest}" = "latest" ]; then
    tag="$(gh release view --repo "$repository" --json tagName --jq .tagName 2>/dev/null || true)"
else
    tag="v${VERSION#v}"
fi
if [ -z "$tag" ]; then
    echo "No release found"
    from_source
    exit 0
fi

version="${tag#v}"
name="faultline-$version-$target"
case "$target" in *windows*) archive="$name.zip" ;; *) archive="$name.tar.gz" ;; esac

work="$(mktemp -d)"
if ! gh release download "$tag" --repo "$repository" --dir "$work" \
    --pattern "$archive" --pattern SHA256SUMS; then
    echo "Release $tag has no $archive"
    from_source
    exit 0
fi

(
    cd "$work"
    grep " $archive\$" SHA256SUMS > expected
    if command -v sha256sum > /dev/null; then
        sha256sum -c expected
    else
        shasum -a 256 -c expected
    fi
)

case "$archive" in
    *.zip) (cd "$work" && 7z x "$archive" > /dev/null) ;;
    *) tar xzf "$work/$archive" -C "$work" ;;
esac
cp "$work/$name"/faultline* "$bin_dir/"
echo "$bin_dir" >> "$GITHUB_PATH"
"$bin_dir"/faultline --version
