#!/usr/bin/env bash
# Hermetic gate aggregation fixtures: copied runner, fake checkers, no builds.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/scripts/lib" "$TMP/bin"
cp "$SCRIPT_DIR/../harden-gates.sh" "$TMP/scripts/"
# Every checker is replaced, including nested fixture scripts.
while IFS= read -r path; do
    [ "$path" != ./scripts/harden-gates.sh ] || continue
    mkdir -p "$TMP/$(dirname "$path")"
    cat > "$TMP/$path" <<'STUB'
#!/usr/bin/env bash
name=$(basename "$0")
echo "$name" >> "$FIXTURE_ROOT/calls"
case "$name" in
    check-prose.sh | check-knowhow-coupling.sh | check_knowhow_refs_test.sh) exit 1 ;;
    mirror-push-gate.sh) sleep 1; touch "$FIXTURE_ROOT/mirror-installed" ;;
    check-mirror-pushers.sh) test -f "$FIXTURE_ROOT/mirror-installed" ;;
esac
STUB
    chmod +x "$TMP/$path"
done < <(grep -oE '\./scripts/[a-zA-Z0-9_./-]+\.sh' "$TMP/scripts/harden-gates.sh")
printf '#!/bin/sh\nexit 0\n' > "$TMP/bin/make"
chmod +x "$TMP/bin/make"
git -C "$TMP" init -q -b main
git -C "$TMP" config user.email fixture@example.invalid
git -C "$TMP" config user.name Fixture
printf '# baseline\n' > "$TMP/scripts/recover-billing.py"
cat > "$TMP/scripts/recover-billing_test.py" <<'PYTHON'
from pathlib import Path
import os
Path(os.environ['FIXTURE_ROOT'], 'billing-ran').touch()
raise SystemExit(19)
PYTHON
git -C "$TMP" add scripts
git -C "$TMP" commit -qm baseline
git -C "$TMP" checkout -qb fixture
export FIXTURE_ROOT="$TMP"
rc=0
PATH="$TMP/bin:$PATH" bash "$TMP/scripts/harden-gates.sh" > "$TMP/out" 2>&1 || rc=$?
[ "$rc" = 1 ]
for check in check-prose.sh check-knowhow-coupling.sh check_knowhow_refs_test.sh check_knowhow_generated_test.sh; do
    if ! grep -qx "$check" "$TMP/calls"; then
        echo "FAIL: aggregation omitted $check"; exit 1
    fi
done
[ "$(cat "$TMP/.lucidos/harden-gates/mirror-pushers.exit")" = 0 ]
echo 'PASS: independent failures aggregate and mirror inspection follows installation'

# An unavailable checker must retain its own failure alongside all other results.
rm "$TMP/scripts/check-prose.sh"
printf '#!/bin/sh\nexit 23\n' > "$TMP/scripts/mirror-push-gate.sh"
rm "$TMP/mirror-installed"
rc=0
PATH="$TMP/bin:$PATH" bash "$TMP/scripts/harden-gates.sh" > "$TMP/out" 2>&1 || rc=$?
[ "$rc" = 1 ]
[ "$(cat "$TMP/.lucidos/harden-gates/prose.exit")" != 0 ]
[ "$(cat "$TMP/.lucidos/harden-gates/mirror-push-gate.exit")" = 23 ]
[ "$(cat "$TMP/.lucidos/harden-gates/mirror-pushers.exit")" != 0 ]
[ "$(cat "$TMP/.lucidos/harden-gates/knowhow-generated-fixtures.exit")" = 0 ]
echo 'PASS: missing checker and failed installation retain independent results'

# Only a changed recovery utility selects the Python fixture. Its failure must
# join the independent checker failures rather than hiding or replacing them.
[ ! -f "$TMP/billing-ran" ]
echo 'PASS: unchanged recovery utility does not select its fixture'
printf '# changed\n' >> "$TMP/scripts/recover-billing.py"
rc=0
PATH="$TMP/bin:$PATH" bash "$TMP/scripts/harden-gates.sh" > "$TMP/out" 2>&1 || rc=$?
[ "$rc" = 1 ]
[ -f "$TMP/billing-ran" ]
[ "$(cat "$TMP/.lucidos/harden-gates/recover-billing.exit")" = 19 ]
[ "$(cat "$TMP/.lucidos/harden-gates/prose.exit")" != 0 ]
echo 'PASS: changed recovery fixture failure aggregates with other failures'

# Untracked replacements and renames must not disappear from change discovery.
git -C "$TMP" checkout -- scripts/recover-billing.py
printf '# new utility\n' > "$TMP/scripts/recover-billing-extra.py"
rm "$TMP/billing-ran"
PATH="$TMP/bin:$PATH" bash "$TMP/scripts/harden-gates.sh" > "$TMP/out" 2>&1 || :
[ -f "$TMP/billing-ran" ]
echo 'PASS: an untracked recovery utility selects its fixture'
rm "$TMP/scripts/recover-billing-extra.py" "$TMP/billing-ran"
mv "$TMP/scripts/recover-billing.py" "$TMP/scripts/renamed.py"
PATH="$TMP/bin:$PATH" bash "$TMP/scripts/harden-gates.sh" > "$TMP/out" 2>&1 || :
[ -f "$TMP/billing-ran" ]
echo 'PASS: renaming the utility retains its fixture obligation'

# A missing comparison base cannot certify that a conditional check is unowed.
git -C "$TMP" branch -D main > /dev/null
rc=0
PATH="$TMP/bin:$PATH" bash "$TMP/scripts/harden-gates.sh" > "$TMP/out" 2>&1 || rc=$?
[ "$rc" = 1 ]
[ "$(cat "$TMP/.lucidos/harden-gates/change-discovery.exit")" != 0 ]
grep -q 'cannot discover changed paths' "$TMP/.lucidos/harden-gates/change-discovery.log"
[ "$(cat "$TMP/.lucidos/harden-gates/knowhow-generated-fixtures.exit")" = 0 ]
echo 'PASS: failed change discovery fails closed and retains other checks'
