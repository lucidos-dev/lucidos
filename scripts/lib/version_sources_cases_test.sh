#!/usr/bin/env bash
# Exercise the real version-source gate in a disposable tracked tree.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
mkdir -p "$ROOT/scripts/lib"
cp "$SCRIPT_DIR/version_sources_test.sh" "$SCRIPT_DIR/test_skip.sh" "$ROOT/scripts/lib/"
# Only the writer's presence matters to this gate; it is never run here.
printf '%s\n' 's/^LUCIDOS_DEFAULT_VERSION=' > "$ROOT/scripts/release.sh"
printf "/result\n" > "$ROOT/.gitignore"
git -C "$ROOT" init -q
for version in 8.7.6 8.7.6-rc.2; do
    printf '%s\n' "$version" > "$ROOT/RELEASE"
    printf 'LUCIDOS_DEFAULT_VERSION="%s"\n' "$version" > "$ROOT/install.sh"
    git -C "$ROOT" add .
    bash "$ROOT/scripts/lib/version_sources_test.sh" > "$ROOT/result"
done
printf 'A planted release literal: 8.7.6-rc.2\n' > "$ROOT/guide.md"
git -C "$ROOT" add guide.md
if bash "$ROOT/scripts/lib/version_sources_test.sh" > "$ROOT/result"; then
    echo 'FAIL: planted rc literal escaped the real scan' >&2; exit 1
fi
grep -q 'hardcoded in' "$ROOT/result"
rm "$ROOT/guide.md"
git -C "$ROOT" add -u
for version in 8.7.6-beta 8.7 8.7.6-rc.0 8.7.6-rc.01; do
    printf '%s\n' "$version" > "$ROOT/RELEASE"
    printf 'LUCIDOS_DEFAULT_VERSION="%s"\n' "$version" > "$ROOT/install.sh"
    if bash "$ROOT/scripts/lib/version_sources_test.sh" > "$ROOT/result"; then
        echo "FAIL: malformed version $version passed" >&2; exit 1
    fi
    grep -q 'not an x.y.z' "$ROOT/result"
done
printf 'version_sources_cases: passed\n'
