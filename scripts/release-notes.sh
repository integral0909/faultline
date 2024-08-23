#!/bin/sh
# Prints the CHANGELOG.md section for one version, without its heading:
#
#   scripts/release-notes.sh 0.2.0
#
# Exits with an error when the changelog has no section for the version.
set -eu

version="${1:?usage: release-notes.sh VERSION}"
changelog="$(dirname "$0")/../CHANGELOG.md"

if ! section="$(awk -v version="$version" '
    /^## \[/ {
        if (found) exit
        if (index($0, "## [" version "]") == 1) { found = 1; next }
    }
    found { print }
    END { if (!found) exit 1 }
' "$changelog")"; then
    echo "CHANGELOG.md has no section for $version" >&2
    exit 1
fi
printf '%s\n' "$section" | sed -e '/./,$!d'
