#!/bin/bash
# Tests for scripts/lib/release_events.sh — the ReleaseStep* / LucidosReleased
# emit helpers that feed the Release Cockpit app. Validates the cockpit contract:
# the event type, the --summary flag (which the cockpit shows as the step note),
# and the --payload step/version shape the cockpit keys on.
#
# Run: ./scripts/lib/release_events_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# Stub `lucidos` on PATH so the helpers invoke it; the stub records each call's
# argv (one arg per line, NUL-safe enough for our literal args) to $CAPTURE.
STUB_DIR="$(mktemp -d)"
CAPTURE="$STUB_DIR/capture.txt"
cat > "$STUB_DIR/lucidos" <<'STUB'
#!/bin/bash
printf '%s\n' "$@" >> "$CAPTURE"
printf '---\n' >> "$CAPTURE"
exit 0
STUB
chmod +x "$STUB_DIR/lucidos"
export CAPTURE
PATH="$STUB_DIR:$PATH"

# shellcheck source=scripts/lib/release_events.sh
source "$SCRIPT_DIR/release_events.sh"

echo "test: emit_release_step emits the cockpit-contract event + payload"
: > "$CAPTURE"
emit_release_step Started build 0.10.1 "Compiling the app"
out="$(cat "$CAPTURE")"
case "$out" in *"events"*"emit"*"ReleaseStepStarted"*) pass "calls events emit ReleaseStepStarted" ;; *) fail "missing events emit ReleaseStepStarted; got: $out" ;; esac
case "$out" in *"--summary"*"Compiling the app"*) pass "passes note via --summary (cockpit note source)" ;; *) fail "missing --summary note; got: $out" ;; esac
case "$out" in *'{"step":"build","version":"0.10.1"}'*) pass "payload carries step + version the cockpit keys on" ;; *) fail "wrong payload; got: $out" ;; esac

echo ""
echo "test: emit_release_step Failed/Succeeded map to the right event types"
: > "$CAPTURE"
emit_release_step Failed notarize 0.10.1 "notary rejected"
case "$(cat "$CAPTURE")" in *"ReleaseStepFailed"*'{"step":"notarize","version":"0.10.1"}'*) pass "Failed → ReleaseStepFailed for the right step" ;; *) fail "Failed event wrong; got: $(cat "$CAPTURE")" ;; esac

echo ""
echo "test: emit_lucidos_released carries {version, commit, tag}"
: > "$CAPTURE"
emit_lucidos_released 0.10.1 abc1234 v0.10.1 "Lucidos 0.10.1 released"
out="$(cat "$CAPTURE")"
case "$out" in *"LucidosReleased"*) pass "emits LucidosReleased" ;; *) fail "missing LucidosReleased; got: $out" ;; esac
case "$out" in *'{"version":"0.10.1","commit":"abc1234","tag":"v0.10.1"}'*) pass "payload carries version/commit/tag" ;; *) fail "wrong released payload; got: $out" ;; esac

echo ""
echo "test: helpers are best-effort no-ops when lucidos is absent"
( PATH="/usr/bin:/bin"; source "$SCRIPT_DIR/release_events.sh"; emit_release_step Started build 0.0.0 "x" ) >/dev/null 2>&1
if [ $? -eq 0 ]; then pass "no-op (returns 0) when lucidos not on PATH"; else fail "non-zero when lucidos absent"; fi

rm -rf "$STUB_DIR"

echo ""
echo "release_events: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
