#!/bin/bash
# Tests the engine preflight gate (preflight_built_engine and
# publish_after_preflight in scripts/lib/workspace.sh): a built engine that
# fails `lucidos-engine preflight` is never published.
#
# Hermetic: the engine is a stub script in a mktemp directory, and the Postgres
# probes, the password and the publish step are shell functions.
#
# Run: ./scripts/lib/preflight_gate_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

# shellcheck source=workspace.sh
source "$SCRIPT_DIR/workspace.sh"

LOG="$SCRATCH/log"
# What the stub Postgres answers: "present", "absent" or "down".
POSTGRES=present
# shellcheck disable=SC2329 # the stubs are called by the functions under test
shared_pg_psql() {
    case "$POSTGRES" in
        present) echo 1 ;;
        absent) echo ;;
        *) return 1 ;;
    esac
}
# shellcheck disable=SC2329
shared_pg_password() { echo "stub-password"; }
# shellcheck disable=SC2329
publish_all_launch_binaries() { echo publish >> "$LOG"; }

WORKSPACE="$SCRATCH/ws"
mkdir -p "$WORKSPACE/.lucidos"
printf 'API_PORT=5100\nPG_PORT=5444\nPG_DATABASE=lucidos_stub\n' > "$WORKSPACE/.lucidos/ports"

UPLIFT="$SCRATCH/uplift"
mkdir -p "$UPLIFT"

# A stub engine that records how it was called and exits with $1. With a
# second argument it rewrites its own file first, as a concurrent build would.
make_engine() {
    local REPLACE_SELF=":"
    [ -z "${2:-}" ] || REPLACE_SELF="echo '# replaced' >> \"\$0\""
    cat > "$UPLIFT/lucidos-engine" <<STUB
#!/bin/bash
echo "argv: \$*" >> "$LOG"
echo "url: \$DATABASE_URL" >> "$LOG"
echo "workspace: \$LUCIDOS_WORKSPACE" >> "$LOG"
if [ $1 -ne 0 ]; then echo 'Error: "migrations failed: stub reason"' >&2; fi
$REPLACE_SELF
exit $1
STUB
    chmod +x "$UPLIFT/lucidos-engine"
}

run_gate() {
    : > "$LOG"
    publish_after_preflight "$UPLIFT" "$SCRATCH/launch" "" > "$SCRATCH/out" 2>&1
}

echo "test: a failing preflight blocks the publish and fails the build"
make_engine 1
run_gate
RC=$?
if [ "$RC" -ne 0 ] && ! grep -qx publish "$LOG" && grep -q "^ERROR: .*stub reason" "$SCRATCH/out"; then
    pass "non-zero exit, nothing published, the reason is on an ERROR line"
else
    fail "exit $RC, log: $(tr '\n' ';' < "$LOG") out: $(cat "$SCRATCH/out")"
fi

echo "test: a passing preflight publishes"
make_engine 0
run_gate
RC=$?
if [ "$RC" -eq 0 ] && grep -qx publish "$LOG"; then
    pass "published after the preflight passed"
else
    fail "exit $RC, log: $(tr '\n' ';' < "$LOG")"
fi

echo "test: the preflight reads the workspace database, and the password stays out of argv"
if grep -qx "argv: preflight" "$LOG" \
    && grep -qx "url: postgres://lucidos:stub-password@localhost:5444/lucidos_stub" "$LOG" \
    && grep -qx "workspace: $WORKSPACE" "$LOG"; then
    pass "database and workspace arrive through the environment"
else
    fail "log: $(tr '\n' ';' < "$LOG")"
fi

echo "test: a workspace with no database skips the preflight and still publishes"
make_engine 1
POSTGRES=absent
run_gate
RC=$?
if [ "$RC" -eq 0 ] && grep -qx publish "$LOG" && ! grep -q "^argv:" "$LOG" \
    && grep -q "Preflight skipped" "$SCRATCH/out"; then
    pass "skipped with a notice"
else
    fail "exit $RC, log: $(tr '\n' ';' < "$LOG") out: $(cat "$SCRATCH/out")"
fi
POSTGRES=present

echo "test: an unreachable Postgres warns loudly and still publishes"
make_engine 1
POSTGRES=down
run_gate
RC=$?
if [ "$RC" -eq 0 ] && grep -qx publish "$LOG" && ! grep -q "^argv:" "$LOG" \
    && grep -q "^WARNING: Preflight skipped: Postgres did not answer" "$SCRATCH/out"; then
    pass "unchecked, and said so"
else
    fail "exit $RC, log: $(tr '\n' ';' < "$LOG") out: $(cat "$SCRATCH/out")"
fi
POSTGRES=present

echo "test: an engine replaced during the preflight is not published"
make_engine 0 replace
run_gate
RC=$?
if [ "$RC" -ne 0 ] && ! grep -qx publish "$LOG" && grep -q "changed while the preflight ran" "$SCRATCH/out"; then
    pass "refused, nothing published"
else
    fail "exit $RC, log: $(tr '\n' ';' < "$LOG") out: $(cat "$SCRATCH/out")"
fi

echo "test: a workspace with no ports file skips the preflight"
mv "$WORKSPACE/.lucidos/ports" "$WORKSPACE/.lucidos/ports.off"
run_gate
RC=$?
mv "$WORKSPACE/.lucidos/ports.off" "$WORKSPACE/.lucidos/ports"
if [ "$RC" -eq 0 ] && grep -qx publish "$LOG" && ! grep -q "^argv:" "$LOG"; then
    pass "skipped, then published"
else
    fail "exit $RC, log: $(tr '\n' ';' < "$LOG")"
fi

echo "test: build_or_find_engine never publishes an engine that fails the preflight"
CHECKOUT="$SCRATCH/checkout"
mkdir -p "$CHECKOUT/target/debug/deps"
UPLIFT="$CHECKOUT/target/debug"
make_engine 1
: > "$LOG"
# shellcheck disable=SC2329 # the stubs are called by build_or_find_engine
(
    set -e
    unset RELEASE
    lsof() { return 1; }
    run_engine_cargo_build() { echo build >> "$LOG"; }
    prune_stale_debug_objects() { :; }
    publish_launch_binaries() { echo publish-launch >> "$LOG"; }
    published_build_state() { echo current; }
    BUILD=1
    PROJECT_DIR="$CHECKOUT"
    build_or_find_engine
) > "$SCRATCH/build.out" 2>&1
RC=$?
if [ "$RC" -ne 0 ] && grep -qx build "$LOG" && ! grep -q "^publish" "$LOG" \
    && grep -q "ERROR: the built engine cannot boot" "$SCRATCH/build.out"; then
    pass "the build failed and no binary moved"
else
    fail "exit $RC, log: $(tr '\n' ';' < "$LOG") out: $(cat "$SCRATCH/build.out")"
fi

echo ""
echo "preflight_gate_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
