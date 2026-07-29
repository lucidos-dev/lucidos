#!/bin/bash
# Tests for scripts/lib/ports.sh.
# Run: ./scripts/lib/ports_test.sh
#
# Three test suites in one file:
#   - host safety: this file's own guarantee that it cannot signal a process it
#     didn't spawn (see the kill shim below).
#   - is_protected_host_pid: the guard that keeps CC subprocesses from killing
#     their own host engine when a test script invokes ports.sh.
#   - allocate_ports: policy tests — default allocation, stability, explicit
#     override (env + lucidos.toml), precedence, collision walk-forward,
#     validation, --engine-only short-circuit.
#
# `port_is_free`, `docker` and `lsof` are stubbed for the WHOLE file so it can
# run anywhere without touching real ports or resolving real pids — stubbing
# `lsof` per test is what let this suite kill the machine's live dev engine
# twice on 2026-07-28 (ADR 0025). The is_protected_host_pid tests use a real
# long-running process so `kill -0 <pid>` succeeds, and the ancestor-arm tests
# use this process's genuine ppid chain.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Sandbox HOME so the global registry (~/.lucidos/port-registry) is isolated.
SANDBOX="$(mktemp -d)"
# Refusal log for the kill shim below. A FILE, not a shell variable: ports.sh
# runs kill_unprotected_pids on the right-hand side of a pipe, so a variable
# assigned inside the shim would be written in a subshell and lost.
KILL_SHIM_LOG="$(mktemp "${TMPDIR:-/tmp}/lucidos_ports_test_kills.XXXXXX")"
trap 'rm -rf "$SANDBOX"; rm -f "$KILL_SHIM_LOG"; kill $LIVE_PID 2>/dev/null || true' EXIT
export HOME="$SANDBOX"

# ── host safety: this file must not be able to signal the machine ───────
# On 2026-07-28 this suite killed the machine's live dev engine twice. The
# allocate_ports tests stubbed `port_is_free` but NOT `lsof`, so
# _port_is_ours_or_free resolved the real pid listening on 5173 and handed it
# to _try_reclaim_stale_lucidos_on_port, which SIGUSR1'd it — the engine's
# legitimate stop signal. The suite-level `lsof` stub further down closes that
# hole; this shim is the backstop for the next stub someone forgets.
#
# `kill` is a bash builtin, but a function of the same name wins, and
# `command kill` still reaches the builtin for the calls we allow:
#   kill -0 <pid>  — a pure liveness probe, never lethal. ports.sh leans on it
#                    (is_protected_host_pid, _acquire_registry_lock, the
#                    reclaim escalation guard), so it always passes through.
#   anything else  — allowed only for a pid this file spawned or synthesized
#                    (own_pid). Every other target is refused, logged loudly,
#                    and recorded for the end-of-run assertion. A missing stub
#                    must cost a red test run, not the user's engine.
TEST_OWNED_PIDS=""

own_pid() { TEST_OWNED_PIDS="$TEST_OWNED_PIDS $1"; }

kill_shim_violations() { tr '\n' ' ' < "$KILL_SHIM_LOG" 2>/dev/null; }
clear_kill_shim_violations() { : > "$KILL_SHIM_LOG"; }

kill() {
    local arg sig="" pids="" pid
    for arg in "$@"; do
        case "$arg" in
            -*) sig="$arg" ;;
            *)  pids="$pids $arg" ;;
        esac
    done
    # Liveness probe — harmless by definition, and ports.sh can't work without it.
    if [ "$sig" = "-0" ]; then
        command kill "$@"
        return $?
    fi
    for pid in $pids; do
        case " $TEST_OWNED_PIDS " in
            *" $pid "*) ;;
            *)
                printf '%s\n' "${sig:--TERM}:$pid" >> "$KILL_SHIM_LOG"
                echo "  ports_test: BLOCKED lethal kill ${sig:--TERM} $pid — not a pid this test spawned" >&2
                return 1
                ;;
        esac
    done
    command kill "$@"
}

# Reset any inherited overrides
unset LUCIDOS_VITE_PORT VITE_PORT_OVERRIDE 2>/dev/null || true
# allocate_ports reads ENGINE_ONLY via ${ENGINE_ONLY:-}; initialize to empty so
# the test runs under `set -u`.
export ENGINE_ONLY=""

# A real long-running process so `kill -0 <pid>` succeeds — needed for the
# pidfile-scan branch which requires the recorded PID to actually be alive.
sleep 600 &
LIVE_PID=$!
disown "$LIVE_PID" 2>/dev/null || true
own_pid "$LIVE_PID"

# shellcheck source=ports.sh
source "$SCRIPT_DIR/ports.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# ── stubs (for allocate_ports tests) ───────────────────────────────────
# By default no ports are occupied. Tests can override OCCUPIED_PORTS to
# simulate squatters.
OCCUPIED_PORTS=""
port_is_free() {
    local port="$1"
    case " $OCCUPIED_PORTS " in
        *" $port "*) return 1 ;;
        *) return 0 ;;
    esac
}

# Stub docker so the PG-container detection path doesn't shell out.
# shellcheck disable=SC2329 # a seam: invoked indirectly by allocate_ports, not from this file
docker() { return 1; }

# Stub `lsof` for the WHOLE file — not per test. ports.sh identifies a port's
# occupier with `lsof -ti :<port> -sTCP:LISTEN`, and an unstubbed call names a
# REAL host process, which the reclaim path then signals (the 2026-07-28
# incident; ADR 0025). Drive it off the same OCCUPIED_PORTS state the
# port_is_free stub uses so the two can never disagree, and hand back a
# synthetic pid above the OS pid ceiling so it matches nothing alive.
#
# Real `lsof -ti` exits non-zero and prints nothing when no socket matches;
# ports.sh branches on the empty output, so mirror both halves.
#
# The synthetic pid counts as test-owned for the kill shim: ports.sh legitimately
# signals a squatter it believes it found (kill_unprotected_pids on the pinned-port
# path), and that is the behaviour under test. It names nothing on the machine, so
# the signal goes nowhere. Anything the shim blocks is therefore a pid that did NOT
# come from this stub — i.e. a real leak.
FAKE_OCCUPIER_PID=999901
own_pid "$FAKE_OCCUPIER_PID"
lsof() {
    local arg port=""
    for arg in "$@"; do
        case "$arg" in :*) port="${arg#:}" ;; esac
    done
    case " $OCCUPIED_PORTS " in
        *" $port "*) echo "$FAKE_OCCUPIER_PID"; return 0 ;;
    esac
    return 1
}

# ── helpers ────────────────────────────────────────────────────────────
reset_env() {
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID
    rm -rf "$HOME/workspaces"
}

write_pidfile() {
    local ws="$1"
    local kind="$2"   # engine or frontend
    local pid="$3"
    local dir="$HOME/workspaces/$ws/.lucidos"
    mkdir -p "$dir"
    echo "$pid" > "$dir/$kind.pid"
}

reset_state() {
    # `${SANDBOX:?}` — an empty SANDBOX would make these `rm -rf /*` and
    # `rm -rf /.lucidos`. Same reason the kill shim exists: nothing in this
    # file gets to reach outside its own sandbox, deliberately or by accident.
    rm -rf "${SANDBOX:?}"/* 2>/dev/null || true
    rm -rf "${SANDBOX:?}"/.lucidos 2>/dev/null || true
    OCCUPIED_PORTS=""
    unset LUCIDOS_VITE_PORT VITE_PORT_OVERRIDE 2>/dev/null || true
    export ENGINE_ONLY=""
    unset API_PORT VITE_PORT PG_PORT 2>/dev/null || true
}

make_workspace() {
    local ws="$1"
    mkdir -p "$ws/.lucidos"
    echo "$ws"
}

# ═══════════════════════════════════════════════════════════════════════
# Suite 0: the file's own host-safety scaffolding
# ═══════════════════════════════════════════════════════════════════════

# ── Test 0: the suite-level kill shim refuses foreign pids ─────────────
# Runs FIRST, because every test after it relies on the shim to be the last
# line of defence if a stub goes missing. Without this assertion the shim
# could silently rot into a no-op and nothing would notice until the next
# time the suite took down the machine's engine.
test_kill_shim_refuses_foreign_pid() {
    echo "test: the suite kill shim refuses a lethal signal to a non-test-owned pid"
    clear_kill_shim_violations

    # 999999 is above the OS pid ceiling, so it names nothing — the point is
    # that the shim refuses on OWNERSHIP, before any signal is attempted.
    if kill -USR1 999999 2>/dev/null; then
        fail "kill shim allowed -USR1 to a pid this file never spawned"
    else
        pass "kill shim refused -USR1 to a pid this file never spawned"
    fi

    case "$(kill_shim_violations)" in
        *"-USR1:999999"*) pass "refusal was recorded for the end-of-run assertion" ;;
        *) fail "refusal was not recorded — the end-of-run assertion would miss a real leak" ;;
    esac

    # kill -0 must still reach the host: ports.sh's liveness checks depend on it,
    # and blocking it would silently break the pidfile-scan and lock-reclaim arms.
    if kill -0 "$LIVE_PID" 2>/dev/null; then
        pass "kill -0 still passes through to a live pid"
    else
        fail "kill -0 was blocked — ports.sh liveness checks would break"
    fi

    # A test-owned pid must still be signalable, or the EXIT trap can't clean up.
    if kill -0 "$LIVE_PID" 2>/dev/null && kill -CONT "$LIVE_PID" 2>/dev/null; then
        pass "a test-owned pid is still signalable"
    else
        fail "kill -CONT to the test's own pid $LIVE_PID was refused"
    fi

    # This probe was deliberate — don't let it fail the end-of-run assertion.
    clear_kill_shim_violations
}

# ═══════════════════════════════════════════════════════════════════════
# Suite 1: is_protected_host_pid
# ═══════════════════════════════════════════════════════════════════════

# ── Test 1: LUCIDOS_HOST_PID env var protects that PID ─────────────────
test_protects_host_pid_from_env_var() {
    echo "test: LUCIDOS_HOST_PID protects the named PID"
    reset_env
    export LUCIDOS_HOST_PID="$LIVE_PID"
    if is_protected_host_pid "$LIVE_PID"; then
        pass "is_protected_host_pid($LIVE_PID) returned 0"
    else
        fail "expected 0 for LUCIDOS_HOST_PID match, got $?"
    fi
}

# ── Test 2: LUCIDOS_FRONTEND_PID env var protects that PID ─────────────
test_protects_frontend_pid_from_env_var() {
    echo "test: LUCIDOS_FRONTEND_PID protects the named PID"
    reset_env
    export LUCIDOS_FRONTEND_PID="$LIVE_PID"
    if is_protected_host_pid "$LIVE_PID"; then
        pass "is_protected_host_pid($LIVE_PID) returned 0"
    else
        fail "expected 0 for LUCIDOS_FRONTEND_PID match, got $?"
    fi
}

# ── Test 3: PID from another workspace's engine.pid is protected ───────
# Mirrors the original incident: CC running in workspace "dev" invokes a
# test that runs ports.sh for workspace "e2e-test". The dev engine PID
# isn't in the e2e-test invocation's env, but it IS on disk under
# ~/workspaces/dev/.lucidos/engine.pid. We must protect it anyway.
test_protects_pid_from_other_workspace_pidfile() {
    echo "test: another workspace's engine.pid protects that PID"
    reset_env
    write_pidfile "dev" "engine" "$LIVE_PID"
    if is_protected_host_pid "$LIVE_PID"; then
        pass "scanned ~/workspaces/dev/.lucidos/engine.pid"
    else
        fail "expected 0 for pidfile-scan match, got $?"
    fi
}

# ── Test 4: frontend.pid on disk is also protected ─────────────────────
test_protects_pid_from_other_workspace_frontend_pidfile() {
    echo "test: another workspace's frontend.pid protects that PID"
    reset_env
    write_pidfile "myws" "frontend" "$LIVE_PID"
    if is_protected_host_pid "$LIVE_PID"; then
        pass "scanned ~/workspaces/myws/.lucidos/frontend.pid"
    else
        fail "expected 0 for frontend pidfile-scan match, got $?"
    fi
}

# ── Test 5: arbitrary PID is NOT protected ─────────────────────────────
# Picking a high random PID that's almost certainly not alive and not in
# any pidfile.
test_does_not_protect_arbitrary_pid() {
    echo "test: arbitrary PID is not protected"
    reset_env
    if is_protected_host_pid 999999; then
        fail "is_protected_host_pid(999999) unexpectedly returned 0"
    else
        pass "is_protected_host_pid(999999) returned non-zero"
    fi
}

# ── Test 6: stale pidfile (recorded PID dead) does NOT protect ─────────
# The pidfile-scan branch requires `kill -0 <pid>` to succeed. A stale
# pidfile pointing to a dead PID is meaningless — and we mustn't refuse
# to kill an unrelated process just because it happens to recycle that
# PID number now. Only LIVE protected processes count.
test_stale_pidfile_does_not_protect() {
    echo "test: stale pidfile does not protect a recycled PID"
    reset_env
    write_pidfile "ghost" "engine" 999999
    if is_protected_host_pid 999999; then
        fail "stale pidfile (999999) unexpectedly protected"
    else
        pass "stale pidfile did not protect"
    fi
}

# ── Test 7: empty PID is not protected (defensive) ─────────────────────
test_empty_pid_is_not_protected() {
    echo "test: empty PID is treated as not protected"
    reset_env
    if is_protected_host_pid ""; then
        fail "empty PID unexpectedly returned 0"
    else
        pass "empty PID returned non-zero"
    fi
}

# ── Test 8: pidfile match still requires liveness even when env unset ──
# Ensures the env-var branch and pidfile-scan branch are independent —
# the pidfile scan should NOT match a dead PID even if neither env var
# is set (covers a regression where the early-return shape forgot the
# kill -0 check on the pidfile path).
test_no_env_vars_no_match_for_dead_pidfile_pid() {
    echo "test: no env vars + dead pidfile PID → not protected"
    reset_env
    write_pidfile "ghost2" "engine" 999998
    if is_protected_host_pid 999998; then
        fail "dead pidfile (999998) protected despite no env vars"
    else
        pass "dead pidfile did not protect"
    fi
}

# ── Test 8a: an ANCESTOR pid is protected with env unset and HOME sandboxed ──
# The 2026-07-28 regression, reproduced exactly. Both of the guard's original
# arms are caller-owned state, and this file defeats both by design:
# reset_env unsets LUCIDOS_HOST_PID/LUCIDOS_FRONTEND_PID, and the sandboxed
# HOME hides every real ~/workspaces/*/.lucidos/engine.pid. The guard failed
# open and the reclaim path SIGUSR1'd the machine's live engine.
#
# The ancestor arm is not caller-owned: a process cannot unset its own
# parentage. Uses the GENUINE ppid chain — no stubbing — because a stubbed
# chain would prove nothing about the real one.
test_ancestor_pid_is_protected_without_env_or_home() {
    echo "test: an ancestor pid is protected with env vars unset and HOME sandboxed"
    reset_env

    if is_protected_host_pid "$$"; then
        pass "our own pid $$ is protected"
    else
        fail "our own pid $$ is not protected"
    fi

    local parent
    parent="$(ps -o ppid= -p $$ 2>/dev/null | tr -d '[:space:]')"
    if [ -z "$parent" ]; then
        fail "could not resolve our own parent pid"
        return
    fi

    # Nothing names this pid: no env var, no pidfile (real or sandboxed). If it
    # is protected, it is protected because it is an ancestor.
    if is_protected_host_pid "$parent"; then
        pass "parent pid $parent protected via the ancestor arm alone"
    else
        fail "parent pid $parent NOT protected — the guard still fails open"
    fi

    # One level is not enough: in the incident the engine sat two levels above
    # the shell the test ran in (script → bash → claude → engine).
    local grandparent
    grandparent="$(ps -o ppid= -p "$parent" 2>/dev/null | tr -d '[:space:]')"
    if [ -n "$grandparent" ] && [ "$grandparent" -gt 1 ] 2>/dev/null; then
        if is_protected_host_pid "$grandparent"; then
            pass "grandparent pid $grandparent protected — the walk goes past depth 1"
        else
            fail "grandparent pid $grandparent NOT protected — the walk stops too early"
        fi
    fi
}

# ── Test 8b: the reclaim path refuses an ancestor, cmdline match or not ──
# The exact call that killed the engine: _try_reclaim_stale_lucidos_on_port saw
# `*lucidos-engine*` in the occupier's cmdline, decided it was a stale orphan,
# and sent SIGUSR1 — the engine's legitimate stop signal.
#
# Only the CMDLINE lookup is faked, so the ancestor walk stays real. (Same
# dispatch shape as the `*ppid=*` ps stub in wait_for_engine_shutdown_test.sh.)
# Deliberately no real binary named lucidos-engine is spawned, and no host
# process is assumed to be at any particular pid.
test_reclaim_refuses_ancestor_with_engine_cmdline() {
    echo "test: reclaim refuses an ancestor whose cmdline looks like lucidos-engine"
    reset_env
    clear_kill_shim_violations
    OCCUPIED_PORTS=""

    local parent
    parent="$(ps -o ppid= -p $$ 2>/dev/null | tr -d '[:space:]')"
    if [ -z "$parent" ]; then
        fail "could not resolve our own parent pid"
        return
    fi

    ps() {
        case "$*" in
            *"-o command="*) echo "/path/to/target/debug/lucidos-engine --workspace demo" ;;
            *) command ps "$@" ;;
        esac
    }

    if _try_reclaim_stale_lucidos_on_port 5173 "$parent" >/dev/null 2>&1; then
        fail "reclaim reported success against ancestor pid $parent"
    else
        pass "reclaim refused ancestor pid $parent despite the engine-shaped cmdline"
    fi

    unset -f ps

    # The refusal must happen BEFORE any signal — a blocked-by-the-shim kill is
    # still a bug, it just isn't a fatal one.
    case "$(kill_shim_violations)" in
        "") pass "no signal was attempted against the ancestor" ;;
        *)  fail "reclaim tried to signal an ancestor: $(kill_shim_violations)" ;;
    esac
    clear_kill_shim_violations
}

# ── Test 8c: the pidfile scan is not confined to $HOME ─────────────────
# The ancestor arm only reaches processes this one descends from, so a SIBLING
# workspace's engine still relies on the pidfile scan. That scan globbed
# "$HOME"/workspaces/* only — and reassigning HOME, exactly what this file does
# to sandbox the port registry, hid every sibling engine from it. The scan now
# also walks the home recorded in the password database, which $HOME can't
# override.
#
# _LUCIDOS_PASSWD_HOME is overridden rather than resolved here, so the test
# proves the SCAN consults a second root without depending on the real machine
# having workspaces, and without writing anything into the real home.
test_pidfile_scan_is_not_confined_to_home() {
    echo "test: the pidfile scan also covers the password-database home"
    reset_env

    local other="$SANDBOX/passwd-home"
    mkdir -p "$other/workspaces/sibling/.lucidos"
    echo "$LIVE_PID" > "$other/workspaces/sibling/.lucidos/engine.pid"

    local saved_home="$_LUCIDOS_PASSWD_HOME"
    local saved_flag="$_LUCIDOS_PASSWD_HOME_RESOLVED"
    _LUCIDOS_PASSWD_HOME="$other"
    _LUCIDOS_PASSWD_HOME_RESOLVED=1

    # reset_env removed $HOME/workspaces, so a match can only come from the
    # second root — and LIVE_PID is a child of this script, not an ancestor,
    # so the ancestor arm can't be the one answering either.
    if is_protected_host_pid "$LIVE_PID"; then
        pass "a sibling engine pidfile outside \$HOME protected pid $LIVE_PID"
    else
        fail "pid $LIVE_PID not protected — the scan is still confined to \$HOME"
    fi

    # The second root is liveness-gated exactly like $HOME: a stale pidfile
    # naming a recycled pid must not protect an unrelated process.
    echo "999997" > "$other/workspaces/sibling/.lucidos/engine.pid"
    if is_protected_host_pid 999997; then
        fail "a dead pid in the second root was protected — the kill -0 gate was lost"
    else
        pass "the second root is liveness-gated like \$HOME"
    fi

    _LUCIDOS_PASSWD_HOME="$saved_home"
    _LUCIDOS_PASSWD_HOME_RESOLVED="$saved_flag"
    rm -rf "$other"
}

# ── Test 8d: the password-database home is parsed exactly ──────────────
# A misparsed root is worse than no second root: the scan walks a path that
# doesn't exist and the arm silently stops protecting, which is the failure
# mode the whole change exists to remove. Two shapes to get right — a value
# containing a space (a field split would truncate it) and a line that doesn't
# look like the expected key (must be discarded, not used).
test_passwd_home_is_parsed_exactly() {
    echo "test: the password-database home parse keeps spaces and rejects junk"
    local saved_home="$_LUCIDOS_PASSWD_HOME"
    local saved_flag="$_LUCIDOS_PASSWD_HOME_RESOLVED"
    # Stub the Linux fallback too, so a CI box can't resolve a real home here.
    getent() { return 1; }

    dscl() { echo "NFSHomeDirectory: /Users/Anne Doe"; }
    _LUCIDOS_PASSWD_HOME=""
    _LUCIDOS_PASSWD_HOME_RESOLVED=""
    _ensure_passwd_home
    if [ "$_LUCIDOS_PASSWD_HOME" = "/Users/Anne Doe" ]; then
        pass "a home path containing a space survives the parse"
    else
        fail "expected '/Users/Anne Doe', got '$_LUCIDOS_PASSWD_HOME' — a truncated root scans the wrong tree"
    fi

    dscl() { echo "NFSHomeDirectory: not-an-absolute-path"; }
    _LUCIDOS_PASSWD_HOME=""
    _LUCIDOS_PASSWD_HOME_RESOLVED=""
    _ensure_passwd_home
    if [ -z "$_LUCIDOS_PASSWD_HOME" ]; then
        pass "a non-absolute parse result is discarded"
    else
        fail "kept a bogus root '$_LUCIDOS_PASSWD_HOME'"
    fi

    unset -f dscl getent
    _LUCIDOS_PASSWD_HOME="$saved_home"
    _LUCIDOS_PASSWD_HOME_RESOLVED="$saved_flag"
}

# ═══════════════════════════════════════════════════════════════════════
# Suite 2: allocate_ports
# ═══════════════════════════════════════════════════════════════════════

# ── Test 9: default allocation uses offset 0 ───────────────────────────
test_default_allocation_uses_offset_0() {
    reset_state
    echo "test: default allocation uses offset 0 (API=3000, Vite=5173, PG=5432)"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-a")"

    if allocate_ports "$ws" > /dev/null 2>&1; then
        if [ "$API_PORT" = "3000" ] && [ "$VITE_PORT" = "5173" ] && [ "$PG_PORT" = "5432" ]; then
            pass "default ports assigned"
        else
            fail "expected 3000/5173/5432, got $API_PORT/$VITE_PORT/$PG_PORT"
        fi
    else
        fail "allocate_ports failed"
    fi
}

# ── Test 10: allocation is stable across calls ─────────────────────────
test_allocation_is_stable_across_calls() {
    reset_state
    echo "test: allocation is stable across repeated calls"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-b")"

    allocate_ports "$ws" > /dev/null 2>&1
    local first_vite="$VITE_PORT"
    allocate_ports "$ws" > /dev/null 2>&1
    local second_vite="$VITE_PORT"

    if [ "$first_vite" = "$second_vite" ]; then
        pass "ports stable across calls ($first_vite)"
    else
        fail "ports changed: first=$first_vite, second=$second_vite"
    fi
}

# ── Test 11: two workspaces get different offsets ──────────────────────
test_two_workspaces_get_different_offsets() {
    reset_state
    echo "test: two workspaces get different offsets"
    local wsa wsb
    wsa="$(make_workspace "$SANDBOX/ws-c")"
    wsb="$(make_workspace "$SANDBOX/ws-d")"

    allocate_ports "$wsa" > /dev/null 2>&1
    local a_vite="$VITE_PORT"
    allocate_ports "$wsb" > /dev/null 2>&1
    local b_vite="$VITE_PORT"

    if [ "$a_vite" != "$b_vite" ]; then
        pass "got distinct vite ports: a=$a_vite, b=$b_vite"
    else
        fail "both workspaces got vite port $a_vite"
    fi
}

# ── Test 12: explicit port override via LUCIDOS_VITE_PORT env var ──────
test_explicit_port_override_via_env_var() {
    reset_state
    echo "test: LUCIDOS_VITE_PORT env var pins the vite port"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-e")"

    export LUCIDOS_VITE_PORT=5273
    allocate_ports "$ws" > /dev/null 2>&1

    if [ "$VITE_PORT" = "5273" ]; then
        pass "vite port = $VITE_PORT (override honored)"
    else
        fail "expected VITE_PORT=5273, got $VITE_PORT"
    fi

    # API and PG should shift by the same offset (100): API=3100, PG=5532
    if [ "$API_PORT" = "3100" ]; then
        pass "API port shifted to $API_PORT (offset 100)"
    else
        fail "expected API_PORT=3100, got $API_PORT"
    fi

    if [ "$PG_PORT" = "5532" ]; then
        pass "PG port shifted to $PG_PORT"
    else
        fail "expected PG_PORT=5532, got $PG_PORT"
    fi
}

# ── Test 13: explicit port override via lucidos.toml ───────────────────
test_explicit_port_override_via_lucidos_toml() {
    reset_state
    echo "test: lucidos.toml [ports] vite = N pins the vite port"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-f")"

    cat > "$ws/lucidos.toml" <<EOF
[ports]
vite = 5300
EOF

    allocate_ports "$ws" > /dev/null 2>&1

    if [ "$VITE_PORT" = "5300" ]; then
        pass "lucidos.toml honored: VITE_PORT=$VITE_PORT"
    else
        fail "expected VITE_PORT=5300, got $VITE_PORT"
    fi
}

# ── Test 14: env var beats lucidos.toml ────────────────────────────────
test_env_var_beats_lucidos_toml() {
    reset_state
    echo "test: LUCIDOS_VITE_PORT env var beats lucidos.toml"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-g")"

    cat > "$ws/lucidos.toml" <<EOF
[ports]
vite = 5300
EOF
    export LUCIDOS_VITE_PORT=5400

    allocate_ports "$ws" > /dev/null 2>&1

    if [ "$VITE_PORT" = "5400" ]; then
        pass "env var won: VITE_PORT=$VITE_PORT"
    else
        fail "expected VITE_PORT=5400 (env), got $VITE_PORT"
    fi
}

# ── Test 14a: the collision walk resolves occupiers through the stub ───
# The walk is where the incident started: test_collision_walks_forward marks
# 5173 occupied, and pre-fix `lsof` was unstubbed, so _port_is_ours_or_free
# handed the reclaim path the REAL pid listening on 5173 — the user's engine.
# Lock the property directly rather than trusting that every future test
# remembers to keep the stub: whatever occupier the walk sees must be the
# synthetic one.
test_collision_walk_resolves_occupier_through_the_stub() {
    reset_state
    echo "test: the collision walk resolves occupiers through the suite lsof stub, never the host"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-lsof-stub")"

    # Exactly the state test_collision_walks_forward sets up.
    OCCUPIED_PORTS="5173"

    OCCUPIER_PID=""
    if _port_is_ours_or_free 5173 "$ws" >/dev/null 2>&1; then
        fail "_port_is_ours_or_free called 5173 free while OCCUPIED_PORTS names it"
    else
        pass "_port_is_ours_or_free reported 5173 occupied"
    fi

    if [ "$OCCUPIER_PID" = "$FAKE_OCCUPIER_PID" ]; then
        pass "occupier is the synthetic pid $FAKE_OCCUPIER_PID — the stub is in force"
    else
        fail "occupier resolved to '${OCCUPIER_PID:-empty}', not $FAKE_OCCUPIER_PID — real lsof reached the host"
    fi
}

# ── Test 15: collision walks forward to next free offset ───────────────
test_collision_walks_forward() {
    reset_state
    echo "test: collision on the default port walks forward to the next free offset"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-h")"

    # Simulate a foreign process holding 5173 (a foreign-process-on-5173 case)
    OCCUPIED_PORTS="5173"

    allocate_ports "$ws" > /dev/null 2>&1

    # Expect offset 1 (vite=5174). Cannot land on 5173.
    if [ "$VITE_PORT" -ge "5174" ]; then
        pass "walked forward: VITE_PORT=$VITE_PORT (avoided 5173)"
    else
        fail "expected VITE_PORT≥5174, got $VITE_PORT"
    fi

    if [ "$VITE_PORT" != "5173" ]; then
        pass "did not pick collided port"
    else
        fail "VITE_PORT landed on collided 5173"
    fi
}

# ── Test 16: collision walk persists in registry ───────────────────────
test_collision_walk_persists() {
    reset_state
    echo "test: collision-bumped offset persists across calls"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-i")"

    # First call hits collision on 5173 → walks to 5174 (or higher)
    OCCUPIED_PORTS="5173"
    allocate_ports "$ws" > /dev/null 2>&1
    local first_vite="$VITE_PORT"

    # Second call: no collision (squatter gone), but registry should remember
    OCCUPIED_PORTS=""
    allocate_ports "$ws" > /dev/null 2>&1
    local second_vite="$VITE_PORT"

    if [ "$first_vite" = "$second_vite" ]; then
        pass "persisted offset reused: $second_vite"
    else
        fail "first=$first_vite, second=$second_vite — drift"
    fi
}

# ── Test 17: an occupied PINNED port is a hard error, never a walk ─────
# A pinned offset (lucidos.toml [ports] vite, or LUCIDOS_VITE_PORT) is
# authoritative — walking off it defeats the point of pinning, and the silent
# drift is what used to change a workspace's URL on every restart. So a genuine
# foreign squatter on a pinned port must fail loudly rather than pick another.
#
# This test asserted the OPPOSITE ("override collision walks forward") until
# 2026-07-28. It was written 2026-05-18; ports.sh adopted never-drift-off-a-pin
# on 2026-06-01 (dd0a238c3) and the test was never updated, so it had been
# failing for two months — invisible, because the suite could not be run
# without risking the machine's engine (ADR 0025). Now it locks the real policy.
test_pinned_port_collision_refuses_to_walk() {
    reset_state
    echo "test: a foreign squatter on a pinned port is a hard error, not a walk"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-j")"

    export LUCIDOS_VITE_PORT=5273
    OCCUPIED_PORTS="5273"

    # Capture stderr to a file so the function runs in the parent shell
    # (a $() subshell would lose the exported VITE_PORT).
    local errfile="$SANDBOX/test17.err"
    allocate_ports "$ws" 2> "$errfile" 1>/dev/null
    local rc=$?

    if [ "$rc" != "0" ]; then
        pass "allocate_ports refused the occupied pin (rc=$rc)"
    else
        fail "allocate_ports returned 0 and drifted to VITE_PORT=${VITE_PORT:-unset} — a pin must never walk"
    fi

    case "$(cat "$errfile")" in
        *"refusing to walk forward off a pinned port"*)
            pass "error explains the refusal" ;;
        *)  fail "error should say it refuses to walk off the pin, got: $(cat "$errfile")" ;;
    esac

    if [ -z "${VITE_PORT:-}" ]; then
        pass "no port exported, so no caller can use a drifted one"
    else
        fail "VITE_PORT=$VITE_PORT was exported despite the failure"
    fi

    if [ ! -f "$ws/.lucidos/ports" ]; then
        pass "no ports file written on the refusal path"
    else
        fail "a ports file was written for a port allocation that failed"
    fi
}

# ── Test 18: chosen ports are logged to stderr ─────────────────────────
test_chosen_ports_are_logged() {
    reset_state
    echo "test: chosen ports are loudly logged to stderr"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-k")"

    local out
    out=$(allocate_ports "$ws" 2>&1 1>/dev/null)

    case "$out" in
        *"5173"*) pass "log mentions the chosen port" ;;
        *) fail "expected '5173' in stderr, got: $out" ;;
    esac
}

# ── Test 19: lucidos.toml with quoted vite value ───────────────────────
test_lucidos_toml_with_quoted_vite_value() {
    reset_state
    echo "test: lucidos.toml with vite = \"5320\" (quoted) is honored as int"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-l")"

    cat > "$ws/lucidos.toml" <<EOF
[ports]
vite = "5320"
EOF

    allocate_ports "$ws" > /dev/null 2>&1

    if [ "$VITE_PORT" = "5320" ]; then
        pass "quoted toml value parsed: VITE_PORT=$VITE_PORT"
    else
        fail "expected VITE_PORT=5320, got $VITE_PORT"
    fi
}

# ── Test 20: non-numeric LUCIDOS_VITE_PORT is rejected ────────────────
test_non_numeric_env_var_is_rejected() {
    reset_state
    echo "test: LUCIDOS_VITE_PORT=abc is rejected with a clear error"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-m")"

    export LUCIDOS_VITE_PORT=abc
    local errfile="$SANDBOX/test20.err"
    if allocate_ports "$ws" 2> "$errfile" 1>/dev/null; then
        fail "expected allocate_ports to fail on non-numeric override, got rc=0 VITE_PORT=${VITE_PORT:-unset}"
    else
        pass "non-numeric override rejected (rc != 0)"
    fi
    case "$(cat "$errfile")" in
        *LUCIDOS_VITE_PORT*) pass "error message names LUCIDOS_VITE_PORT" ;;
        *) fail "error should name LUCIDOS_VITE_PORT, got: $(cat "$errfile")" ;;
    esac
}

# ── Test 21: out-of-range LUCIDOS_VITE_PORT is rejected ───────────────
test_out_of_range_env_var_is_rejected() {
    reset_state
    echo "test: LUCIDOS_VITE_PORT=100 (< 5173) is rejected"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-n")"

    export LUCIDOS_VITE_PORT=100
    local errfile="$SANDBOX/test21.err"
    if allocate_ports "$ws" 2> "$errfile" 1>/dev/null; then
        fail "expected allocate_ports to fail on port <5173, got rc=0 API=${API_PORT:-unset}"
    else
        pass "out-of-range override rejected (rc != 0)"
    fi
}

# ── Test 22: bogus toml vite value is rejected ────────────────────────
test_bogus_toml_value_is_rejected() {
    reset_state
    echo "test: lucidos.toml with vite = \"abc\" is rejected"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-o")"

    cat > "$ws/lucidos.toml" <<EOF
[ports]
vite = "abc"
EOF
    local errfile="$SANDBOX/test22.err"
    if allocate_ports "$ws" 2> "$errfile" 1>/dev/null; then
        fail "expected allocate_ports to fail on non-numeric toml value"
    else
        pass "non-numeric toml value rejected"
    fi
}

# ── Test 23a: stale lucidos-engine on registered port is reclaimed ─────
# Regression: work workspace was at offset 5 in the registry, but each
# restart silently drifted because allocate_ports walked past any port
# that lsof reported occupied — including ports held by our own crashed/
# orphaned lucidos-engine that nothing else would ever clean up. Once
# walked, the new offset was persisted and the port had drifted forever.
# Fix: _port_is_ours_or_free first asks _try_reclaim_stale_lucidos_on_port
# to free the port; only genuine foreign occupiers cause a walk.
test_stale_lucidos_engine_reclaimed_no_drift() {
    reset_state
    echo "test: stale lucidos-engine on the registered port is reclaimed → no drift"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-stale-self")"

    # First call: clean — gets offset 0 (vite=5173, api=3000).
    allocate_ports "$ws" > /dev/null 2>&1
    [ "$VITE_PORT" = "5173" ] || { fail "setup: expected VITE_PORT=5173, got $VITE_PORT"; return; }

    # Now both ports look occupied to port_is_free (squatter). The suite-level
    # lsof stub sets OCCUPIER_PID deterministically off that same OCCUPIED_PORTS
    # state, so the reclaim path is exercised everywhere rather than only on a
    # machine that happens to have a listener. (This test used to install its
    # OWN `lsof` stub and then `unset -f lsof`, which tore down any suite-level
    # stub for every test that ran after it — leaving them on the real lsof.)
    # The reclaim helper, when called, simulates a successful kill by
    # clearing OCCUPIED_PORTS — i.e. proves allocate_ports *consults* the
    # reclaim path before walking, and trusts its success.
    OCCUPIED_PORTS="5173 3000"
    # shellcheck disable=SC2329 # a seam: invoked indirectly by allocate_ports, not from this file
    _try_reclaim_stale_lucidos_on_port() {
        local port="$1"
        OCCUPIED_PORTS="$(echo " $OCCUPIED_PORTS " | sed "s/ $port / /g" | sed 's/^ //; s/ $//')"
        return 0
    }

    allocate_ports "$ws" > /dev/null 2>&1

    unset -f _try_reclaim_stale_lucidos_on_port

    if [ "$VITE_PORT" = "5173" ]; then
        pass "reclaim succeeded, stayed on offset 0 (VITE_PORT=$VITE_PORT)"
    else
        fail "drifted to VITE_PORT=$VITE_PORT (expected 5173 after reclaim)"
    fi
}

# ── Test 23b: foreign occupier still walks (no regression) ─────────────
# When the reclaim helper refuses (genuinely foreign process), the walk-
# forward fallback must still kick in so the workspace eventually gets a
# port. Mirrors the policy in test_collision_walks_forward but explicit
# about the new reclaim integration.
test_foreign_occupier_still_walks_after_reclaim_refuses() {
    reset_state
    echo "test: foreign occupier + reclaim refuses → walks forward (no regression)"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-foreign")"

    OCCUPIED_PORTS="5173"
    # shellcheck disable=SC2329 # a seam: invoked indirectly by allocate_ports, not from this file
    _try_reclaim_stale_lucidos_on_port() { return 1; }

    allocate_ports "$ws" > /dev/null 2>&1

    unset -f _try_reclaim_stale_lucidos_on_port

    if [ "$VITE_PORT" -ge "5174" ]; then
        pass "walked past foreign occupier: VITE_PORT=$VITE_PORT"
    else
        fail "expected VITE_PORT≥5174, got $VITE_PORT"
    fi
}

# ── Test 23: allocate_ports in --engine-only mode returns 0 without
# trying to free VITE_PORT. Pre-fix, the kill-guard refused to signal
# the workspace's own engine on VITE_PORT and allocate_ports returned 1
# with "Port still occupied", leaving the old engine in place and
# silently breaking the engine-only restart flow.
test_allocate_ports_engine_only_short_circuits() {
    reset_state
    echo "test: allocate_ports --engine-only short-circuits past cleanup"
    local ws="$HOME/workspaces/restart-target"
    mkdir -p "$ws/.lucidos"
    # The workspace's "engine" — a live sleep we expect to survive.
    sleep 600 &
    local fake_engine_pid=$!
    disown "$fake_engine_pid" 2>/dev/null || true
    own_pid "$fake_engine_pid"
    echo "$fake_engine_pid" > "$ws/.lucidos/engine.pid"

    # Run in --engine-only mode. Should return 0 immediately and NOT
    # invoke any of the lsof/kill machinery.
    # shellcheck disable=SC2030 # scoping ENGINE_ONLY to this subshell IS the point — later tests must not see it
    (
        export ENGINE_ONLY=1
        allocate_ports "$ws"
    )
    local rc=$?

    if [ $rc -ne 0 ]; then
        fail "allocate_ports returned $rc in --engine-only mode (expected 0)"
    elif ! kill -0 "$fake_engine_pid" 2>/dev/null; then
        fail "fake engine pid $fake_engine_pid was killed by allocate_ports"
    elif [ ! -f "$ws/.lucidos/ports" ]; then
        fail "allocate_ports did not write $ws/.lucidos/ports"
    else
        pass "short-circuited; fake engine survived; ports file written"
    fi

    kill "$fake_engine_pid" 2>/dev/null || true
}

# ── Test 24: .lucidos/ports records the user-facing port (post-swap),
# not the raw 3000-range API_PORT. Consumers (status.sh, frontend
# bookmarks) read this file as "the URL".
test_ports_file_records_user_facing_port() {
    reset_state
    echo "test: .lucidos/ports records the user-facing port (post-swap), not the raw API port"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-userfacing")"

    allocate_ports "$ws" > /dev/null 2>&1

    local ports_file="$ws/.lucidos/ports"
    [ -f "$ports_file" ] || { fail "ports file not written"; return; }

    # offset 0 → vite=5173 (user-facing), api=3000 (would be the raw value
    # we DON'T want to see in the file).
    local recorded_api recorded_vite
    recorded_api=$(awk -F= '/^API_PORT=/  { print $2 }' "$ports_file")
    recorded_vite=$(awk -F= '/^VITE_PORT=/ { print $2 }' "$ports_file")

    if [ "$recorded_api" = "5173" ]; then
        pass "API_PORT in file = $recorded_api (user-facing)"
    else
        fail "expected API_PORT=5173 (user-facing), got $recorded_api (would leak raw 3000-range to consumers)"
    fi

    if [ "$recorded_vite" = "5173" ]; then
        pass "VITE_PORT in file = $recorded_vite (user-facing)"
    else
        fail "expected VITE_PORT=5173, got $recorded_vite"
    fi

    if [ "$recorded_api" = "$recorded_vite" ]; then
        pass "both keys agree on the same user-facing port"
    else
        fail "API_PORT ($recorded_api) and VITE_PORT ($recorded_vite) disagree — consumers will pick the wrong one"
    fi
}

# ── Test 25: --engine-only mode writes the user-facing port to
# .lucidos/ports without depending on swap_ports running afterwards.
test_ports_file_correct_in_engine_only_mode() {
    reset_state
    echo "test: --engine-only mode writes user-facing port to .lucidos/ports (no dependency on swap_ports)"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-engine-only")"

    # shellcheck disable=SC2031 # a fresh subshell; ShellCheck is carrying over the deliberate scoping above
    (
        export ENGINE_ONLY=1
        allocate_ports "$ws"
    ) > /dev/null 2>&1

    local recorded_api recorded_vite
    recorded_api=$(awk -F= '/^API_PORT=/  { print $2 }' "$ws/.lucidos/ports")
    recorded_vite=$(awk -F= '/^VITE_PORT=/ { print $2 }' "$ws/.lucidos/ports")

    if [ "$recorded_api" = "5173" ] && [ "$recorded_vite" = "5173" ]; then
        pass "--engine-only ports file: API_PORT=VITE_PORT=$recorded_api"
    else
        fail "expected both = 5173 (user-facing), got API=$recorded_api VITE=$recorded_vite"
    fi
}

# ── Test 26: registry_save under N concurrent writers preserves every
# entry exactly once. Each writer must serialize behind the registry
# lock so no read-modify-write window can lose a sibling's append.
test_registry_save_handles_concurrent_writers() {
    reset_state
    echo "test: registry_save handles concurrent writers without corruption"

    local N=20
    local i
    for i in $(seq 1 $N); do
        ( registry_save "/sandbox/ws-$i" "$i" ) &
    done
    wait

    local lines unique_workspaces
    lines=$(wc -l < "$LUCIDOS_PORT_REGISTRY" | tr -d ' ')
    unique_workspaces=$(awk -F'\t' '{print $1}' "$LUCIDOS_PORT_REGISTRY" | sort -u | wc -l | tr -d ' ')

    if [ "$lines" = "$N" ]; then
        pass "registry has exactly $N entries after $N concurrent writers"
    else
        fail "expected $N entries, got $lines (corruption or lost writes)"
    fi

    if [ "$unique_workspaces" = "$N" ]; then
        pass "all $N workspaces are present (no clobber)"
    else
        fail "expected $N unique workspaces, got $unique_workspaces"
    fi
}

# ── Test 27: registry_save failure propagates through allocate_ports.
# A lock-acquire timeout or mktemp failure must bubble up — silently
# swallowing it leaves the file written but the offset unpersisted, so
# the next restart can't find the entry and picks a fresh one (drift).
test_registry_save_failure_propagates() {
    reset_state
    echo "test: registry_save failure propagates as allocate_ports non-zero"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-fail-prop")"

    # Force registry_save to fail. The override stays in effect only for
    # this test (reset_state restores the function table on next run via
    # re-source) — but reset_state doesn't re-source, so unset explicitly.
    # shellcheck disable=SC2329 # a seam: invoked indirectly by allocate_ports, not from this file
    registry_save() { return 1; }

    if allocate_ports "$ws" > /dev/null 2>&1; then
        fail "expected allocate_ports to return non-zero when registry_save fails"
    else
        pass "allocate_ports propagated registry_save failure"
    fi

    unset -f registry_save
}

# ── run all ────────────────────────────────────────────────────────────
test_kill_shim_refuses_foreign_pid
test_protects_host_pid_from_env_var
test_protects_frontend_pid_from_env_var
test_protects_pid_from_other_workspace_pidfile
test_protects_pid_from_other_workspace_frontend_pidfile
test_does_not_protect_arbitrary_pid
test_stale_pidfile_does_not_protect
test_empty_pid_is_not_protected
test_no_env_vars_no_match_for_dead_pidfile_pid
test_ancestor_pid_is_protected_without_env_or_home
test_reclaim_refuses_ancestor_with_engine_cmdline
test_pidfile_scan_is_not_confined_to_home
test_passwd_home_is_parsed_exactly
test_default_allocation_uses_offset_0
test_allocation_is_stable_across_calls
test_two_workspaces_get_different_offsets
test_explicit_port_override_via_env_var
test_explicit_port_override_via_lucidos_toml
test_env_var_beats_lucidos_toml
test_collision_walk_resolves_occupier_through_the_stub
test_collision_walks_forward
test_collision_walk_persists
test_pinned_port_collision_refuses_to_walk
test_chosen_ports_are_logged
test_lucidos_toml_with_quoted_vite_value
test_non_numeric_env_var_is_rejected
test_out_of_range_env_var_is_rejected
test_bogus_toml_value_is_rejected
test_stale_lucidos_engine_reclaimed_no_drift
test_foreign_occupier_still_walks_after_reclaim_refuses
test_allocate_ports_engine_only_short_circuits
test_ports_file_records_user_facing_port
test_ports_file_correct_in_engine_only_mode
test_registry_save_handles_concurrent_writers
test_registry_save_failure_propagates

# ── final host-safety assertion ────────────────────────────────────────
# Anything still on the refusal log means a test reached for a pid it doesn't
# own — i.e. a stub went missing and only the shim stood between this suite and
# a live host process. That must be a red run, not a green one with a warning
# scrolled off the top.
echo "test: no unexpected kill-shim refusals during the run"
UNEXPECTED_KILLS="$(kill_shim_violations)"
case "$UNEXPECTED_KILLS" in
    "") pass "no test attempted a lethal signal outside its own sandbox" ;;
    *)  fail "kill shim blocked unexpected lethal signal(s): $UNEXPECTED_KILLS — a stub is missing" ;;
esac

echo ""
echo "Results: $PASS passed, $FAIL failed"
exit "$FAIL"
