#!/usr/bin/env bash
# Run release.sh's actual argument and changelog checks without release effects.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/test_skip.sh
source "$SCRIPT_DIR/test_skip.sh"
test_skip_unless_present "$SCRIPT_DIR/../.." scripts/release.sh
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
awk '
    /^if \[\[ "\$PHASE" == "prep-preflight" && "\$VERSION" == "auto"/ { copying=1 }
    copying && /^if \[\[ -n "\$PR_NUMBER"/ { exit }
    copying { print }
' "$SCRIPT_DIR/../release.sh" > "$ROOT/check.sh"
[ -s "$ROOT/check.sh" ]
check() (
    # shellcheck disable=SC2034 # Read by the extracted release code.
    PHASE="$1" VERSION="$2" RC_BUILD="${3:-0}" PR_NUMBER="" BASE_REF_OVERRIDE="" DEFER_NOTARIZATION=0
    # shellcheck disable=SC2329 # Called by the extracted release code.
    fail() { echo "$*" >&2; exit 1; }
    # shellcheck source=/dev/null
    source "$ROOT/check.sh"
)
check verify-build 8.7.6
check verify-build 8.7.6 1
if check oneshot 8.7.6 1; then echo "FAIL: --rc accepted outside Phase A" >&2; exit 1; fi
check resume-notarize 8.7.6-rc.2
check abandon 8.7.6-rc.10
check prep-preflight auto
for phase in verify-build publish-verified push-rc oneshot; do
    if check "$phase" 8.7.6-rc.2 > "$ROOT/error" 2>&1; then
        echo "FAIL: $phase accepted an explicit rc" >&2; exit 1
    fi
done
for version in 8.7.6-beta 8.7.6-rc.01 8.7; do
    if check resume-notarize "$version" > "$ROOT/error" 2>&1; then
        echo "FAIL: accepted malformed build $version" >&2; exit 1
    fi
done
# Exercise the full condition, including the capture used to match the version.
awk '
    /if \[\[ "\$FIRST_NONBLANK" =~/ { print; print "exit 0; else exit 1; fi"; found=1; exit }
    END { if (!found) exit 1 }
' "$SCRIPT_DIR/../release.sh" > "$ROOT/header.sh"
VERSION=8.7.6-rc.10 FIRST_NONBLANK='## v8.7.6-rc.10' bash "$ROOT/header.sh"
VERSION=8.7.6 FIRST_NONBLANK='## v8.7.6' bash "$ROOT/header.sh"
if VERSION=8.7.6 FIRST_NONBLANK='## v8.7.6-rc.10' bash "$ROOT/header.sh"; then
    echo 'FAIL: rc header accepted for the final' >&2; exit 1
fi
printf 'release_version: passed\n'
