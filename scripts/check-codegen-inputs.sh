#!/usr/bin/env bash
# Fail when a workspace crate the codegen renderers depend on is missing from
# CODEGEN_INPUT_ROOTS (scripts/lib/codegen.sh). The renderer's own dep-info
# check catches this only when it compiles, and a codegen cache hit never
# compiles. A new crate is outside the fingerprint, so the hit stays a hit
# until main's build misses and fails there. This reads `cargo metadata`, so
# it runs in about a second.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/codegen.sh
source "$ROOT/scripts/lib/codegen.sh"

UNCOVERED=$(codegen_uncovered_crates "$ROOT")
if [ -n "$UNCOVERED" ]; then
    echo "check-codegen-inputs: the renderers depend on crates the codegen fingerprint does not cover:" >&2
    printf '%s\n' "$UNCOVERED" | sed 's/^/  /' >&2
    echo "check-codegen-inputs: add each to CODEGEN_INPUT_ROOTS (scripts/lib/codegen.sh)" >&2
    exit 1
fi
