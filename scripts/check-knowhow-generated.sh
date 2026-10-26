#!/usr/bin/env bash
# Compare a fresh rendering with committed output; never rewrite the checkout.
# Missing, empty, malformed or stale committed output fails closed.
set -euo pipefail
[ "$#" -eq 0 ] || { echo 'check-knowhow-generated: takes no arguments.' >&2; exit 1; }
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
codegen_render_to "$ROOT" "$OUT/rendered"
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
