#!/usr/bin/env bash
# Compare a rendering with committed output; never rewrite the checkout.
# Missing, empty, malformed or stale committed output fails closed.
#
# The rendering is the codegen cache's entry for this tree's fingerprint when
# there is one: codegen_check_inputs verified the render that wrote it, so it
# is byte-identical to a fresh one. A miss renders, which compiles the engine.
# --cached-only fails on a miss instead, for a caller that must not compile.
set -euo pipefail
CACHED_ONLY=""
case "$*" in
    "") ;;
    --cached-only) CACHED_ONLY=1 ;;
    *) echo 'check-knowhow-generated: takes only --cached-only.' >&2; exit 1 ;;
esac
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
# shellcheck source=scripts/lib/codegen.sh
source "$ROOT/scripts/lib/codegen.sh"
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT
MANIFEST=docs/generated/agent-surface.json
if ! git show "HEAD:$MANIFEST" > "$OUT/committed.json"; then
    echo "knowhow-generated: committed $MANIFEST is missing." >&2
    exit 1
fi
[ -s "$ROOT/$MANIFEST" ] && [ -s "$OUT/committed.json" ] || {
    echo "knowhow-generated: $MANIFEST is missing or empty." >&2; exit 1;
}
if CACHED="$(codegen_cached "$ROOT" "$(codegen_fingerprint "$ROOT")")"; then
    echo "knowhow-generated: comparing with the cached rendering for this tree"
    ln -s "$CACHED" "$OUT/rendered"
elif [ -n "$CACHED_ONLY" ]; then
    echo "knowhow-generated: no cached rendering for this tree. Run ./scripts/codegen.sh first." >&2
    exit 1
else
    codegen_render_to "$ROOT" "$OUT/rendered"
fi
python3 - "$OUT" "$MANIFEST" <<'PY'
import json
import pathlib
import sys

out = pathlib.Path(sys.argv[1])
committed = out / 'committed.json'
fresh = out / 'rendered' / sys.argv[2]
for path in (committed, fresh, pathlib.Path(sys.argv[2])):
    surface = json.loads(path.read_text())
    if (surface.get('schema_version') != 1 or not surface.get('cli', {}).get('commands')
            or not surface.get('tools', {}).get('offered') or not surface.get('events', {}).get('thread')
            or not surface.get('events', {}).get('system') or not surface.get('events', {}).get('legacy')
            or not surface.get('sdk_exports')):
        raise SystemExit('knowhow-generated: empty or malformed agent surface: ' + str(path))
if committed.read_bytes() != fresh.read_bytes():
    raise SystemExit('knowhow-generated: committed agent-surface.json is stale. Run ./scripts/codegen.sh and commit it.')
print('✓ committed agent surface matches a fresh rendering')
PY
python3 "$ROOT/scripts/lib/codegen_blocks.py" check "$OUT/rendered/$CODEGEN_BLOCKS" "$ROOT"
echo '✓ committed knowhow blocks match a fresh rendering'
