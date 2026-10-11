#!/bin/bash
# Tests for scripts/lib/e2e_gateway.sh (ADR 0385).
#
# Hermetic. The gateway listing is stubbed, and the one gateway this file starts
# is a fake binary under the sandbox that records its environment and sleeps.
# Nothing here reaches a real gateway or signals a process it did not spawn.
#
# Run: ./scripts/lib/e2e_gateway_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
# check <label> <command...>: pass when the command succeeds.
check() {
    local label="$1"
    shift
    if "$@"; then pass "$label"; else fail "$label"; fi
}
not_grep() { ! grep -q "$@"; }
assert_eq() { # <expected> <actual> <label>
    if [ "$1" = "$2" ]; then pass "$3"; else fail "$3 (expected '$1', got '$2')"; fi
}

export E2E_WORKSPACE="$SANDBOX/e2e-test"
mkdir -p "$E2E_WORKSPACE/.lucidos/worktrees"
# shellcheck source=scripts/lib/e2e.sh
source "$SCRIPT_DIR/e2e.sh"

# A listing that answers for one gateway port only, like a machine running the
# dev gateway and nothing else.
LISTING_GATEWAY_PORT="$DEFAULT_DEV_GATEWAY_PORT"
LISTING_ENGINE_PORT=5341
e2e_gateway_listed_port() {
    [ "$1" = "$LISTING_GATEWAY_PORT" ] && [ "$2" = e2e-test ] && echo "$LISTING_ENGINE_PORT"
    return 0
}

test_identity_needs_the_engine_port_listed() {
    echo "test: the identity names a gateway only when it lists the engine's port"
    local got
    got="$(e2e_gateway_identity 5341 5341 5251 e2e-test)"
    assert_eq "5251 e2e-test" "$got" "listed at the engine's port"
    got="$(e2e_gateway_identity 5341 6000 5251 e2e-test)"
    assert_eq "" "$got" "listed at another port gives none"
    got="$(e2e_gateway_identity "" 5341 5251 e2e-test)"
    assert_eq "" "$got" "not listed gives none"
}

test_auto_candidates_are_deduplicated() {
    echo "test: auto mode asks the inherited gateway, then the dev one, once each"
    local got
    got="$(e2e_gateway_auto_candidates 5252 5251 | tr '\n' ' ')"
    assert_eq "5252 5251 " "$got" "inherited first"
    got="$(e2e_gateway_auto_candidates 5251 5251 | tr '\n' ' ')"
    assert_eq "5251 " "$got" "no repeat"
    got="$(e2e_gateway_auto_candidates "" 5251 | tr '\n' ' ')"
    assert_eq "5251 " "$got" "nothing inherited"
}

# Run attach_e2e_gateway in a subshell with a coding-agent session's inherited
# environment, and print what the engine would inherit.
attach_from_session() {
    # shellcheck disable=SC2030 # the subshell is the simulated session: nothing outside reads these
    (
        export WORKSPACE="$E2E_WORKSPACE" ENGINE_PORT="$1"
        export LUCIDOS_GATEWAY_PORT="$2" LUCIDOS_WORKSPACE_ID=dev
        unset LUCIDOS_E2E_GATEWAY LUCIDOS_DEV_GATEWAY_PORT
        attach_e2e_gateway >/dev/null 2>&1
        echo "port=${LUCIDOS_GATEWAY_PORT:-} id=${LUCIDOS_WORKSPACE_ID:-}"
    )
}

test_auto_replaces_the_inherited_dev_identity() {
    echo "test: auto mode gives the engine its own id, never the inherited 'dev'"
    local got
    got="$(attach_from_session 5341 "$DEFAULT_DEV_GATEWAY_PORT")"
    assert_eq "port=$DEFAULT_DEV_GATEWAY_PORT id=e2e-test" "$got" "registered as e2e-test"
}

test_auto_clears_both_when_no_gateway_lists_the_engine() {
    echo "test: auto mode clears both variables when the listing names another port"
    local got
    got="$(attach_from_session 6000 "$DEFAULT_DEV_GATEWAY_PORT")"
    assert_eq "port= id=" "$got" "no gateway identity at all"
}

test_auto_falls_through_to_the_dev_gateway() {
    echo "test: auto mode asks the dev gateway when the inherited one does not list us"
    local got
    got="$(attach_from_session 5341 5252)"
    assert_eq "port=$DEFAULT_DEV_GATEWAY_PORT id=e2e-test" "$got" "found on the dev gateway"
}

test_unknown_mode_is_refused() {
    echo "test: an unknown LUCIDOS_E2E_GATEWAY is refused"
    # shellcheck disable=SC2030,SC2031 # each case scopes its environment to its own subshell
    if (export LUCIDOS_E2E_GATEWAY=shared WORKSPACE="$E2E_WORKSPACE"; attach_e2e_gateway >/dev/null 2>&1); then
        fail "accepted 'shared'"
    else
        pass "refused"
    fi
}

test_own_gateway_env_is_isolated() {
    echo "test: the own gateway's environment stays off the machine-global gateway"
    local env_out
    env_out="$(e2e_gateway_env "$SANDBOX/gw" 40001 /bin/engine /dist)"
    for want in "LUCIDOS_GATEWAY_DATA=$SANDBOX/gw" LUCIDOS_API_PORT=40001 \
        LUCIDOS_GATEWAY_BIND_ADDR=127.0.0.1 LUCIDOS_HOOK_PORT=0; do
        check "$want" grep -qx "$want" <<<"$env_out"
    done
}

test_log_summary_counts_adoptions_and_spawns() {
    echo "test: the stop line says whether the gateway only adopted"
    local log="$SANDBOX/summary.log" got
    printf '%s\n' \
        "[Gateway] adopted the engine already running for 'e2e-test' on :5341" \
        "[Gateway] adopted the engine already running for 'e2e-test' on :5341" \
        "[Gateway] lazy-starting 'e2e-test' on demand" >"$log"
    got="$(e2e_gateway_log_summary "$log")"
    assert_eq "e2e gateway: adopted the engine 2 time(s), started or respawned one 1 time(s)" "$got" "counts both"
    got="$(e2e_gateway_log_summary "$SANDBOX/no-such.log")"
    assert_eq "e2e gateway: adopted the engine 0 time(s), started or respawned one 0 time(s)" "$got" "a missing log reads as zero"
}

# The entry scripts run under `set -e`. A clean gateway log has no start lines,
# so `grep -c` exits 1 there.
test_stop_survives_set_e() {
    echo "test: stopping the gateway completes under set -e"
    local dir got
    dir="$(e2e_gateway_dir)"
    got="$(set -e; stop_e2e_gateway >/dev/null; echo reached)"
    assert_eq reached "$got" "no gateway running"

    mkdir -p "$dir"
    echo "[Gateway] adopted the engine already running for 'e2e-test' on :5341" >"$dir/gateway.log"
    (exec -a lucidos-gateway sleep 30) &
    echo "$!" >"$dir/gateway.pid"
    got="$(set -e; stop_e2e_gateway; echo reached)"
    case "$got" in
        *"adopted the engine 1 time(s), started or respawned one 0 time(s)"*reached) pass "a clean log" ;;
        *) fail "a clean log aborted the stop: '$got'" ;;
    esac
    rm -f "$dir/gateway.log"
}

test_stop_never_signals_a_reused_pid() {
    echo "test: a pidfile naming a process that is not the gateway is never signalled"
    local dir pid
    dir="$(e2e_gateway_dir)"
    mkdir -p "$dir"
    sleep 30 &
    pid=$!
    echo "$pid" >"$dir/gateway.pid"
    stop_e2e_gateway >/dev/null
    check "the unrelated process survives" kill -0 "$pid"
    kill "$pid" 2>/dev/null
    check "the stale pidfile is removed" test ! -e "$dir/gateway.pid"
}

test_own_refuses_a_worktree_engine() {
    echo "test: own mode refuses a coding-agent worktree before starting anything"
    local marker="$SANDBOX/started"
    rm -f "$marker"
    # shellcheck disable=SC2030,SC2031 # each case scopes its environment to its own subshell
    if (
        export WORKSPACE="$E2E_WORKSPACE" ENGINE_PORT=5341 LUCIDOS_E2E_GATEWAY=own
        ENGINE_BIN="/repo/.lucidos/worktrees/t1/.launch/release/x/lucidos-engine"
        GATEWAY_BIN="/usr/bin/touch $marker"
        attach_e2e_gateway >/dev/null 2>&1
    ); then
        fail "own mode ran from a worktree"
    else
        pass "refused"
    fi
    check "no gateway was started" test ! -e "$marker"
}

test_own_starts_an_isolated_gateway_and_stops_it() {
    echo "test: own mode starts a session gateway on its own data dir, then stops it"
    local fake="$SANDBOX/fake-gateway" env_file="$SANDBOX/gateway.env" dir pid out
    cat >"$fake" <<EOF
#!/bin/bash
env >"$env_file"
exec -a lucidos-gateway sleep 30
EOF
    chmod +x "$fake"
    dir="$(e2e_gateway_dir)"
    # shellcheck disable=SC2031 # each case scopes its environment to its own subshell
    out="$(
        export WORKSPACE="$E2E_WORKSPACE" ENGINE_PORT=5341 LUCIDOS_E2E_GATEWAY=own
        export LUCIDOS_GATEWAY_PORT=5251 LUCIDOS_WORKSPACE_ID=dev
        ENGINE_BIN="$SANDBOX/lucidos-engine" GATEWAY_BIN="$fake"
        # Health answers once the fake has written its env; the listing then
        # reads the registry the harness seeded.
        curl() { [ -s "$env_file" ]; }
        # shellcheck disable=SC2329 # called by attach_e2e_gateway in this subshell
        e2e_gateway_listed_port() {
            python3 -c 'import json,sys; print([w["port"] for w in json.load(open(sys.argv[1]))["workspaces"] if w["id"]=="e2e-test"][0])' \
                "$dir/config/workspaces.json"
        }
        attach_e2e_gateway >/dev/null 2>&1 || echo "attach failed"
        echo "port=${LUCIDOS_GATEWAY_PORT:-} id=${LUCIDOS_WORKSPACE_ID:-}"
    )"
    pid="$(cat "$dir/gateway.pid" 2>/dev/null)"

    case "$out" in
        "port=$(cat "$dir/port" 2>/dev/null) id=e2e-test") pass "the engine points at the own gateway as e2e-test" ;;
        *) fail "got '$out'" ;;
    esac
    check "data dir is inside the e2e workspace" grep -qx "LUCIDOS_GATEWAY_DATA=$dir" "$env_file"
    check "the gateway inherits no workspace id" not_grep '^LUCIDOS_WORKSPACE_ID=' "$env_file"
    python3 -c 'import json,sys; w=json.load(open(sys.argv[1]))["workspaces"][0]; sys.exit(0 if w["port"]==5341 and w["autostart"] is False else 1)' \
        "$dir/config/workspaces.json"
    assert_eq 0 "$?" "registry lists the engine's port, autostart off"

    stop_e2e_gateway >/dev/null
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null
        fail "the gateway survived stop_e2e_gateway"
    else
        pass "stopped"
    fi
    check "pidfile removed" test ! -e "$dir/gateway.pid"
}

test_identity_needs_the_engine_port_listed
test_auto_candidates_are_deduplicated
test_auto_replaces_the_inherited_dev_identity
test_auto_clears_both_when_no_gateway_lists_the_engine
test_auto_falls_through_to_the_dev_gateway
test_unknown_mode_is_refused
test_own_gateway_env_is_isolated
test_log_summary_counts_adoptions_and_spawns
test_stop_survives_set_e
test_stop_never_signals_a_reused_pid
test_own_refuses_a_worktree_engine
test_own_starts_an_isolated_gateway_and_stops_it

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
