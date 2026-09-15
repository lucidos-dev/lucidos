#!/bin/bash
# Tests for scripts/lib/preflight_reclaim.sh, the pre-flight engine reclaim the
# nightly runs before its memory gate.
#
# Run: ./scripts/lib/preflight_reclaim_test.sh   (no harness; direct, like
# host_memory_guard_test.sh and e2e_lock_test.sh)

# HERMETIC BY CONSTRUCTION, AND THAT IS LOAD-BEARING.
#
# This library's whole job is to find engines and stop them, so a test that
# reached the real host would stop the machine's live workspaces. Nothing here
# discovers, signals or stops a real process.
#
#   - The process table is a sandbox FILE. Both host seams read it and nothing
#     else, so an empty table means "no engines" and never falls back to the
#     real pgrep. That fallback is what put the host process table into a real
#     kill in webkit_reaper_test.sh (ADR 0025).
#   - scripts/stop.sh is a RECORDING STUB in a sandbox checkout, reached the
#     same way the real one is: RECLAIM_PROJECT_DIR/scripts/stop.sh. So the
#     tests pin the actual invocation, flag included.
#   - `kill` is shimmed as a function and `pgrep`, `ps`, `pkill` and `killall`
#     are shimmed on PATH. Each records a bypass and refuses. The suite fails
#     if any of them was reached.
#   - The memory reading is a queue, never vm_stat.
#   - The CLOCK is a counter and the SLEEP advances it. No test waits out a
#     watch window, and no test depends on how long it took to run.
#   - The gateway logs are sandbox files. The real ones are never opened, and
#     the seam is a function override, not an environment variable that a
#     forgotten export would leave pointing at the packaged log.
#
# Exit codes are captured directly (cmd; rc=$?), never through a masking pipe.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SANDBOX="$(mktemp -d -t lucidos-preflight-reclaim-test.XXXXXX)"
cleanup() { rm -rf "$SANDBOX"; }
trap cleanup EXIT

OUT="$SANDBOX/out"
mkdir -p "$OUT"

export PROCTABLE="$SANDBOX/proctable"
export STOP_LOG="$SANDBOX/stop.log"
export STOP_MODE="sticks"
export CLOCK="$SANDBOX/clock"
export EVENTS="$SANDBOX/events"
export DRAIN_S=8
GWLOG="$SANDBOX/gateway.log"
BYPASS_LOG="$SANDBOX/bypass.log"
: >"$PROCTABLE"
: >"$STOP_LOG"
: >"$BYPASS_LOG"
: >"$EVENTS"
: >"$GWLOG"
printf '1000\n' >"$CLOCK"

# ── the sandbox checkout the library will call stop.sh in ───────────────
FAKE_CHECKOUT="$SANDBOX/checkout"
mkdir -p "$FAKE_CHECKOUT/scripts"
cat >"$FAKE_CHECKOUT/scripts/stop.sh" <<'STUB'
#!/bin/bash
# Recording stub for scripts/stop.sh. Records its argv, then models what the
# real stop did to the host, per STOP_MODE:
#   sticks   the workspace stays down (the real scripts/stop.sh path)
#   drain    the engine exits DRAIN_S seconds later, keeping its pid until then
#   respawn  the gateway supervisor brings it back at once, with a FRESH pid
#   stays    the engine never goes away at all
#   fail     stop.sh itself failed
set -u
printf '%s\n' "$*" >>"$STOP_LOG"
[ "$STOP_MODE" = "fail" ] && exit 1
[ "$STOP_MODE" = "stays" ] && exit 0
ws="$2"
if [ "$STOP_MODE" = "drain" ]; then
    printf '%s\tremove\t-\t-\t%s\t-\n' "$(($(cat "$CLOCK") + DRAIN_S))" "$ws" >>"$EVENTS"
    exit 0
fi
tmp="$(mktemp)"
if [ "$STOP_MODE" = "respawn" ]; then
    awk -F'\t' -v want="$ws" 'BEGIN { OFS = "\t" }
        { if ($3 == want) $1 = $1 + 100000; print }' "$PROCTABLE" >"$tmp"
else
    awk -F'\t' -v want="$ws" '$3 != want' "$PROCTABLE" >"$tmp"
fi
mv "$tmp" "$PROCTABLE"
exit 0
STUB
chmod +x "$FAKE_CHECKOUT/scripts/stop.sh"

# ── host-call shims ─────────────────────────────────────────────────────
# On PATH, so even a subprocess is covered. `command -v pgrep` still resolves,
# which is what the library's own precondition check needs.
SHIM_BIN="$SANDBOX/bin"
mkdir -p "$SHIM_BIN"
for shim in pgrep ps pkill killall; do
    cat >"$SHIM_BIN/$shim" <<SHIMEOF
#!/bin/bash
printf 'PATH %s %s\n' "$shim" "\$*" >>"$BYPASS_LOG"
exit 1
SHIMEOF
    chmod +x "$SHIM_BIN/$shim"
done
export PATH="$SHIM_BIN:$PATH"

# `kill` is a shell builtin, so PATH cannot intercept it here. This function
# can, for every call made in this shell.
# shellcheck disable=SC2329 # a guard: reached only by a regression, never by a passing run
kill() {
    printf 'BUILTIN kill %s\n' "$*" >>"$BYPASS_LOG"
    return 1
}

# ── the library under test ──────────────────────────────────────────────
export RECLAIM_PROJECT_DIR="$FAKE_CHECKOUT"
# shellcheck source=preflight_reclaim.sh
source "$SCRIPT_DIR/preflight_reclaim.sh"

# The fake clock. It lives in a FILE because the library reads it inside command
# substitutions, and a shell variable would not survive that subshell.
_reclaim_now() { cat "$CLOCK"; }

# The only thing that moves time. So a test never waits, and the elapsed numbers
# in the output are exactly what the poll cadence implies.
_reclaim_sleep() {
    printf '%s\n' "$(($(cat "$CLOCK") + $1))" >"$CLOCK"
}

# One sandbox gateway log, never the machine's. An override rather than an
# environment variable: a forgotten export would leave the default pointing at
# the packaged log, which this suite must never open.
_reclaim_gateway_logs() { printf '%s\n' "$GWLOG"; }

# Scheduled changes to the host, applied when the fake clock reaches them. This
# is how a respawn 20 seconds after the stop is modelled without waiting 20
# seconds. Fields: at, kind (add or remove), pid, id, ws, gateway log line.
apply_due_events() {
    [ -s "$EVENTS" ] || return 0
    local now at kind pid id ws line pending
    now="$(cat "$CLOCK")"
    pending=""
    while IFS="$(printf '\t')" read -r at kind pid id ws line; do
        [ -n "$at" ] || continue
        if [ "$now" -lt "$at" ]; then
            pending="$pending$(printf '%s\t%s\t%s\t%s\t%s\t%s' "$at" "$kind" "$pid" "$id" "$ws" "$line")
"
            continue
        fi
        if [ "$kind" = "remove" ]; then
            awk -F'\t' -v want="$ws" '$3 != want' "$PROCTABLE" >"$PROCTABLE.tmp"
            mv "$PROCTABLE.tmp" "$PROCTABLE"
        else
            printf '%s\t%s\t%s\n' "$pid" "$id" "$ws" >>"$PROCTABLE"
            [ "$line" = "-" ] || printf '%s\n' "$line" >>"$GWLOG"
        fi
    done <"$EVENTS"
    printf '%s' "$pending" >"$EVENTS"
}

# Both host seams read the sandbox table and nothing else. An empty table is
# "no engines", never a reason to ask the real host.
_reclaim_engine_pids() {
    apply_due_events
    [ -s "$PROCTABLE" ] || return 0
    awk -F'\t' 'NF { print $1 }' "$PROCTABLE"
}

# A `ps -E` shaped line: argv, then NAME=value pairs. The order matches the real
# thing, so LUCIDOS_WORKSPACE is followed by LUCIDOS_WORKSPACE_ID and a value
# holding a space has to be parsed rather than split on.
_reclaim_proc_env() {
    awk -F'\t' -v want="$1" '
        $1 == want {
            line = "/opt/lucidos/bin/lucidos-engine"
            if ($3 != "-") line = line " LUCIDOS_WORKSPACE=" $3
            if ($2 != "-") line = line " LUCIDOS_WORKSPACE_ID=" $2
            print line " OSLogRateLimit=64 PATH=/usr/bin:/bin"
            exit
        }
    ' "$PROCTABLE"
}

# The available-memory reading, as a queue of pinned values. One per call, so a
# test can pin a before and an after and assert the delta.
#
# The queue lives in a FILE because the library reads it inside a command
# substitution, and a shell variable would not survive that subshell: both calls
# would return the same number and the delta would always be zero.
AVAIL_QUEUE="$SANDBOX/avail-queue"
: >"$AVAIL_QUEUE"
set_avail() { printf '%s' "$*" >"$AVAIL_QUEUE"; }
_host_mem_read_available_gb() {
    local seq head rest
    seq="$(cat "$AVAIL_QUEUE" 2>/dev/null)"
    head="${seq%% *}"
    rest="${seq#* }"
    [ "$rest" = "$seq" ] && rest=""
    printf '%s' "$rest" >"$AVAIL_QUEUE"
    printf '%s' "$head"
}

# ── harness ─────────────────────────────────────────────────────────────
PASS=0
FAIL=0
fail() {
    echo "  FAIL: $*"
    FAIL=$((FAIL + 1))
}
pass() {
    echo "  ok:   $*"
    PASS=$((PASS + 1))
}

assert_eq() {
    if [ "$2" = "$3" ]; then
        pass "$1"
    else
        fail "$1 (want '$3', got '$2')"
    fi
}

assert_contains() {
    case "$2" in
        *"$3"*) pass "$1" ;;
        *) fail "$1 (no '$3' in: $2)" ;;
    esac
}

assert_not_contains() {
    case "$2" in
        *"$3"*) fail "$1 (found '$3' in: $2)" ;;
        *) pass "$1" ;;
    esac
}

reset_host() {
    : >"$PROCTABLE"
    : >"$STOP_LOG"
    : >"$EVENTS"
    : >"$GWLOG"
    printf '1000\n' >"$CLOCK"
    STOP_MODE="sticks"
    DRAIN_S=8
    set_avail "12.00 12.00"
    unset LUCIDOS_RECLAIM_KEEP
    unset LUCIDOS_RECLAIM_SETTLE_S
    unset LUCIDOS_RECLAIM_QUIET_S
    unset LUCIDOS_RECLAIM_DEADLINE_S
}

add_proc() {
    printf '%s\t%s\t%s\n' "$1" "$2" "$3" >>"$PROCTABLE"
}

# A workspace that comes back $1 seconds after the run starts, as pid $2, with
# gateway log line $5 (or "-" for none).
add_comeback() {
    printf '%s\tadd\t%s\t%s\t%s\t%s\n' \
        "$((1000 + $1))" "$2" "$3" "$4" "$5" >>"$EVENTS"
}

elapsed_s() { echo $(($(cat "$CLOCK") - 1000)); }

RC=0
STDOUT=""
STDERR=""
# Redirect to files and read $? straight back. A pipe would report the last
# command's status, so a red run could read as green.
run_reclaim() {
    preflight_reclaim_main >"$OUT/stdout" 2>"$OUT/stderr"
    RC=$?
    STDOUT="$(cat "$OUT/stdout")"
    STDERR="$(cat "$OUT/stderr")"
}

stop_calls() { cat "$STOP_LOG"; }

# ────────────────────────────────────────────────────────────────────────
echo "preflight_reclaim: a non-keep engine is stopped through stop.sh"
reset_host
add_proc 44011 coldrun /home/u/.lucidos/gateway/workspaces/coldrun
run_reclaim
assert_eq "exits 0 once the stop sticks" "$RC" "0"
assert_eq "stop.sh called once, with the env path and -w" \
    "$(stop_calls)" "-w /home/u/.lucidos/gateway/workspaces/coldrun"
assert_contains "names the engine it stopped" "$STDOUT" "stop coldrun (pid 44011)"
assert_contains "the after list is empty" "$STDOUT" "engines up after: 0"

# The path must come out of LUCIDOS_WORKSPACE. A constructed ~/workspaces/<slug>
# is what made the old snippet exit 1 on "Workspace not found" and read clean.
echo "preflight_reclaim: the path comes from LUCIDOS_WORKSPACE, never a layout guess"
reset_host
add_proc 44012 coldrun /var/lucidos/elsewhere/coldrun
run_reclaim
assert_eq "stop.sh got the env path verbatim" \
    "$(stop_calls)" "-w /var/lucidos/elsewhere/coldrun"
assert_not_contains "no ~/workspaces path was invented" "$(stop_calls)" "/workspaces/coldrun"

echo "preflight_reclaim: a workspace path holding a space survives the parse"
reset_host
add_proc 1288 packaged "/home/u/Library/Application Support/com.lucidos.app/workspaces/packaged"
run_reclaim
assert_eq "the whole path reached stop.sh" \
    "$(stop_calls)" "-w /home/u/Library/Application Support/com.lucidos.app/workspaces/packaged"

# Both default keep-list entries, so the packaged one is pinned as well as the
# dev one. Its fixture path deliberately follows no layout, because the script
# reads the path out of the process and never builds one.
echo "preflight_reclaim: a keep-list workspace is never stopped"
reset_host
add_proc 79603 dev /home/u/workspaces/dev
add_proc 1288 personal "/opt/lucidos/packaged root/personal"
run_reclaim
assert_eq "exits 0" "$RC" "0"
assert_eq "stop.sh was never called" "$(stop_calls)" ""
assert_contains "both are still up afterwards" "$STDOUT" "engines up after: 2"
assert_contains "the keep list is on the record" "$STDOUT" "keep list: dev personal"

echo "preflight_reclaim: keep-list matching is exact, so devbox is not dev"
reset_host
add_proc 51001 devbox /home/u/workspaces/devbox
run_reclaim
assert_eq "exits 0" "$RC" "0"
assert_eq "devbox was stopped" "$(stop_calls)" "-w /home/u/workspaces/devbox"

echo "preflight_reclaim: a prefix of a keep-list entry is not kept either"
reset_host
add_proc 51003 de /home/u/workspaces/de
run_reclaim
assert_contains "'de' was stopped" "$(stop_calls)" "-w /home/u/workspaces/de"

echo "preflight_reclaim: an engine whose id cannot be read is left alone"
reset_host
add_proc 60001 - /home/u/workspaces/mystery
run_reclaim
assert_eq "exits 0: an unidentified engine is not a failed reclaim" "$RC" "0"
assert_eq "stop.sh was never called" "$(stop_calls)" ""
assert_contains "the skip is on the record, with the pid" "$STDOUT" "skip pid 60001"
assert_contains "it is counted, not hidden" "$STDOUT" "engines up after: 1 (?:60001)"
# This engine has a readable PATH and an unreadable id. A tab is IFS
# whitespace, so an empty id column would collapse and shift the path into it.
assert_not_contains "the path never lands in the id column" "$STDOUT" "mystery:60001"

echo "preflight_reclaim: a reclaimable engine with no path fails loudly"
reset_host
add_proc 60002 coldrun -
run_reclaim
assert_eq "exits non-zero" "$RC" "1"
assert_eq "stop.sh was never called" "$(stop_calls)" ""
assert_contains "the warning names the workspace and pid" "$STDERR" \
    "WARNING: coldrun (pid 60002) has no LUCIDOS_WORKSPACE"

echo "preflight_reclaim: a survivor is a failure of the step"
reset_host
STOP_MODE="respawn"
add_proc 44011 coldrun /home/u/.lucidos/gateway/workspaces/coldrun
run_reclaim
assert_eq "exits non-zero" "$RC" "1"
assert_contains "the warning names the workspace and its fresh pid" "$STDERR" \
    "WARNING: coldrun came back 0s after stop.sh, as pid 144011"
assert_contains "the verdict is loud" "$STDERR" "VERDICT: FAILED"
assert_contains "the after list shows the respawned pid" "$STDOUT" "engines up after: 1 (coldrun:144011)"

echo "preflight_reclaim: a failing stop.sh is a failure of the step"
reset_host
STOP_MODE="fail"
add_proc 44011 coldrun /home/u/.lucidos/gateway/workspaces/coldrun
run_reclaim
assert_eq "exits non-zero" "$RC" "1"
assert_contains "the warning names stop.sh" "$STDERR" "WARNING: stop.sh failed for coldrun (pid 44011)"

echo "preflight_reclaim: nothing to reclaim exits 0 with no per-engine noise"
reset_host
run_reclaim
assert_eq "exits 0" "$RC" "0"
assert_eq "stderr is silent" "$STDERR" ""
assert_eq "exactly the before and after lines" "$(printf '%s\n' "$STDOUT" | wc -l | tr -d ' ')" "5"
assert_contains "before count" "$STDOUT" "engines up before: 0"
assert_contains "after count" "$STDOUT" "engines up after: 0"
assert_not_contains "nothing was stopped, so nothing is watched" "$STDOUT" "watching"

echo "preflight_reclaim: a keep-only host is quiet too"
reset_host
add_proc 79603 dev /home/u/workspaces/dev
run_reclaim
assert_eq "exactly the before and after lines" "$(printf '%s\n' "$STDOUT" | wc -l | tr -d ' ')" "5"
assert_eq "stderr is silent" "$STDERR" ""

echo "preflight_reclaim: the before and after lines carry the memory delta"
reset_host
set_avail "8.25 11.75"
add_proc 44011 coldrun /home/u/.lucidos/gateway/workspaces/coldrun
run_reclaim
assert_contains "available before" "$STDOUT" "available before: 8.25 GB"
assert_contains "available after, with the delta" "$STDOUT" "available after: 11.75 GB (delta +3.50 GB)"
assert_contains "the watch is announced once a stop was issued" "$STDOUT" "watching for 39s of quiet"

echo "preflight_reclaim: a memory reading it cannot take is never invented"
reset_host
set_avail ""
run_reclaim
assert_contains "before reads unknown" "$STDOUT" "available before: unknown"
assert_contains "after reads unknown, with no delta" "$STDOUT" "available after: unknown"
assert_not_contains "no delta was made up" "$STDOUT" "delta"

echo "preflight_reclaim: the keep list is overridable"
reset_host
add_proc 44011 coldrun /home/u/.lucidos/gateway/workspaces/coldrun
add_proc 79603 dev /home/u/workspaces/dev
LUCIDOS_RECLAIM_KEEP="coldrun"
run_reclaim
assert_eq "only dev was stopped" "$(stop_calls)" "-w /home/u/workspaces/dev"
assert_contains "the override is on the record" "$STDOUT" "keep list: coldrun"

echo "preflight_reclaim: every engine is classified in one pass"
reset_host
add_proc 79603 dev /home/u/workspaces/dev
add_proc 44011 coldrun /home/u/.lucidos/gateway/workspaces/coldrun
add_proc 60001 - /home/u/workspaces/mystery
run_reclaim
assert_eq "exits 0" "$RC" "0"
assert_contains "the before list names all three" "$STDOUT" \
    "engines up before: 3 (dev:79603 coldrun:44011 ?:60001)"
assert_contains "the after list names the two left" "$STDOUT" \
    "engines up after: 2 (dev:79603 ?:60001)"
assert_eq "only the reclaimable one was stopped" \
    "$(stop_calls)" "-w /home/u/.lucidos/gateway/workspaces/coldrun"

# ── the watch window ────────────────────────────────────────────────────
#
# The bug these replace: the script slept 10 seconds, re-scanned once, saw the
# engine gone, and exited 0. The supervisor brought it back at 20 seconds. A
# window shorter than the failure mode it verifies against is not a
# verification, so every case below puts the comeback beyond the old 10.

COLDRUN_WS=/home/u/.lucidos/gateway/workspaces/coldrun
RESPAWN_LINE="09:36:59 [pid:1101] [Gateway] respawning 'coldrun' after 6 missed probe(s) (outcome=Unreachable, alive=false)"
LAZY_LINE="09:37:02 [pid:1101] [Gateway] lazy-starting 'coldrun' on demand"

echo "preflight_reclaim: a supervisor respawn inside the window fails the step"
reset_host
add_proc 44011 coldrun "$COLDRUN_WS"
add_comeback 20 77209 coldrun "$COLDRUN_WS" "$RESPAWN_LINE"
run_reclaim
assert_eq "exits non-zero" "$RC" "1"
assert_contains "names the workspace, the delay and the fresh pid" "$STDERR" \
    "WARNING: coldrun came back 20s after stop.sh, as pid 77209"
assert_contains "says which of the two paths brought it back" "$STDERR" \
    "gateway supervisor respawn, from the gateway log"
assert_contains "quotes the log line itself" "$STDERR" "after 6 missed probe(s)"
assert_contains "the verdict is loud" "$STDERR" "VERDICT: FAILED"
assert_not_contains "and never claims the host went quiet" "$STDOUT" "quiet for"

echo "preflight_reclaim: a client lazy-start inside the window fails it too"
reset_host
add_proc 44011 coldrun "$COLDRUN_WS"
add_comeback 3 77300 coldrun "$COLDRUN_WS" "$LAZY_LINE"
run_reclaim
assert_eq "exits non-zero" "$RC" "1"
assert_contains "the other path is named as itself" "$STDERR" \
    "client lazy-start, from the gateway log"
assert_contains "quotes the log line itself" "$STDERR" "lazy-starting 'coldrun' on demand"

echo "preflight_reclaim: a comeback no log line explains is still a failure"
reset_host
add_proc 44011 coldrun "$COLDRUN_WS"
add_comeback 12 77400 coldrun "$COLDRUN_WS" "-"
run_reclaim
assert_eq "exits non-zero" "$RC" "1"
assert_contains "the comeback is reported anyway" "$STDERR" "coldrun came back 12s after stop.sh"
assert_contains "and the gap is admitted, not papered over" "$STDERR" \
    "no gateway log line names it"

# A log line from BEFORE the stop describes a different episode. Quoting it
# would send the morning reader after a respawn that already happened.
echo "preflight_reclaim: a log line written before the stop is never quoted"
reset_host
add_proc 44011 coldrun "$COLDRUN_WS"
printf '%s\n' "$RESPAWN_LINE" >"$GWLOG"
add_comeback 12 77500 coldrun "$COLDRUN_WS" "-"
run_reclaim
assert_eq "exits non-zero" "$RC" "1"
assert_contains "the stale line is not offered as the cause" "$STDERR" \
    "no gateway log line names it"
assert_not_contains "and is not quoted" "$STDERR" "after 6 missed probe(s)"

echo "preflight_reclaim: a slow drain is waited out, not called a comeback"
reset_host
STOP_MODE="drain"
DRAIN_S=18
add_proc 44011 coldrun "$COLDRUN_WS"
run_reclaim
assert_eq "exits 0 once it finally went and stayed gone" "$RC" "0"
assert_not_contains "an engine keeping its own pid is not back" "$STDERR" "came back"
# The quiet clock starts when the engine LEFT, not when stop.sh returned, so
# the run lasts the drain plus the window. Bounded above by one poll, since the
# window can only be reported on a tick.
QUIET_DEFAULT="$(_reclaim_quiet_s)"
POLL_DEFAULT="$(_reclaim_poll_s)"
WATCHED="$(elapsed_s)"
if [ "$WATCHED" -ge "$((DRAIN_S + QUIET_DEFAULT))" ] &&
    [ "$WATCHED" -lt "$((DRAIN_S + QUIET_DEFAULT + POLL_DEFAULT))" ]; then
    pass "waited the drain then the whole window (${WATCHED}s)"
else
    fail "want ${DRAIN_S}+${QUIET_DEFAULT}s of watching, got ${WATCHED}s"
fi
assert_contains "and says how long the host was quiet" "$STDOUT" "quiet for"

echo "preflight_reclaim: an engine that never leaves fails at the deadline"
reset_host
STOP_MODE="stays"
add_proc 44011 coldrun "$COLDRUN_WS"
run_reclaim
assert_eq "exits non-zero" "$RC" "1"
assert_contains "the give-up is explicit" "$STDERR" "gave up after 60s without 39s of quiet"
assert_contains "and names the knob to raise" "$STDERR" "LUCIDOS_RECLAIM_DEADLINE_S"
assert_contains "the survivor is named too" "$STDERR" \
    "WARNING: coldrun (pid 44011) is up at the end of the watch"

# The old knob's value is exactly the window that hid the bug. Honouring it
# would put the bug back, and ignoring it in silence would hide that.
echo "preflight_reclaim: the retired settle knob is named and not honoured"
reset_host
LUCIDOS_RECLAIM_SETTLE_S=10
add_proc 44011 coldrun "$COLDRUN_WS"
add_comeback 20 77209 coldrun "$COLDRUN_WS" "$RESPAWN_LINE"
run_reclaim
unset LUCIDOS_RECLAIM_SETTLE_S
assert_contains "the retirement is announced" "$STDERR" \
    "LUCIDOS_RECLAIM_SETTLE_S is retired and ignored"
assert_contains "it names what replaced it" "$STDERR" "LUCIDOS_RECLAIM_QUIET_S"
assert_eq "and the 20s comeback is still caught" "$RC" "1"

echo "preflight_reclaim: the window is overridable"
reset_host
LUCIDOS_RECLAIM_QUIET_S=6
LUCIDOS_RECLAIM_POLL_S=3
add_proc 44011 coldrun "$COLDRUN_WS"
run_reclaim
unset LUCIDOS_RECLAIM_QUIET_S LUCIDOS_RECLAIM_POLL_S
assert_eq "exits 0" "$RC" "0"
assert_contains "the shorter window is on the record" "$STDOUT" \
    "watching for 6s of quiet, up to 59s, every 3s"
assert_eq "and it really stopped there" "$(elapsed_s)" "6"

# ── the derivation this window rests on ─────────────────────────────────
#
# The window is not a round number: it is read off the supervisor's pacing and
# the client's boot watchdog. A comment saying so would rot in silence, so this
# reads both files and fails when the arithmetic stops holding. Fail CLOSED: a
# constant that cannot be found is a failure, never a skipped check.
echo "preflight_reclaim: the quiet window still covers the constants it is derived from"
REPO="$(cd "$SCRIPT_DIR/../.." && pwd)"
GW_SRC="$REPO/crates/lucidos-gateway/src/server.rs"
APP_HTML="$REPO/crates/lucidos-app/index.html"

rust_secs() { sed -nE "s/^const $1: Duration = Duration::from_secs\(([0-9]+)\);.*/\1/p" "$GW_SRC" | head -1; }
rust_u32() { sed -nE "s/^const $1: u32 = ([0-9]+);.*/\1/p" "$GW_SRC" | head -1; }

SUPERVISE_INTERVAL="$(rust_secs SUPERVISE_INTERVAL)"
RESPAWN_BACKOFF="$(rust_secs RESPAWN_BACKOFF)"
DEAD_MISS_THRESHOLD="$(rust_u32 DEAD_MISS_THRESHOLD)"
WATCHDOG_MS="$(sed -nE 's/.*setTimeout\(recover, ([0-9]+)\).*/\1/p' "$APP_HTML" | head -1)"

for pair in "SUPERVISE_INTERVAL:$SUPERVISE_INTERVAL" "RESPAWN_BACKOFF:$RESPAWN_BACKOFF" \
    "DEAD_MISS_THRESHOLD:$DEAD_MISS_THRESHOLD" "boot-watchdog-ms:$WATCHDOG_MS"; do
    case "${pair#*:}" in
        '' | *[!0-9]*)
            fail "${pair%%:*} could not be read from source, so the derivation cannot be checked"
            ;;
        *) pass "${pair%%:*} reads ${pair#*:} in source" ;;
    esac
done

# The supervisor's own worst case, plus the client watchdog and its one retry.
NEEDED=$((DEAD_MISS_THRESHOLD * SUPERVISE_INTERVAL + RESPAWN_BACKOFF + 2 * WATCHDOG_MS / 1000))
DEFAULT_QUIET="$(LUCIDOS_RECLAIM_QUIET_S="" _reclaim_quiet_s)"
if [ "$DEFAULT_QUIET" -ge "$NEEDED" ]; then
    pass "the ${DEFAULT_QUIET}s quiet window still covers the ${NEEDED}s a comeback needs"
else
    fail "the window is ${DEFAULT_QUIET}s but a comeback can take ${NEEDED}s; the constants moved"
fi

DEFAULT_DEADLINE="$(LUCIDOS_RECLAIM_DEADLINE_S="" _reclaim_deadline_s)"
if [ "$DEFAULT_DEADLINE" -gt "$DEFAULT_QUIET" ]; then
    pass "the deadline leaves room for the drain before the quiet window can start"
else
    fail "the deadline ($DEFAULT_DEADLINE) cannot fit one quiet window ($DEFAULT_QUIET)"
fi

DEFAULT_POLL="$(LUCIDOS_RECLAIM_POLL_S="" _reclaim_poll_s)"
assert_eq "the poll cadence is the supervisor's own tick" "$DEFAULT_POLL" "$SUPERVISE_INTERVAL"

# ── the default project dir ─────────────────────────────────────────────
# Every test above sets RECLAIM_PROJECT_DIR to the sandbox, so the default
# branch of that parameter expansion is the one value nothing exercises, and the
# one that matters in production. Evaluate it in a clean subshell with the
# variable unset, and pin it to the real repo root the test already knows. This
# fails if the default climbs the wrong number of directories in either
# direction: scripts/ and the root's parent hold no scripts/stop.sh. Sourcing
# the library runs no host call, so this stays hermetic.
echo "preflight_reclaim: the default project dir is the repo root that holds scripts/stop.sh"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DEFAULT_DIR="$(unset RECLAIM_PROJECT_DIR; source "$SCRIPT_DIR/preflight_reclaim.sh" >/dev/null 2>&1; printf '%s' "$RECLAIM_PROJECT_DIR")"
assert_eq "the default resolves to the repo root" "$DEFAULT_DIR" "$REPO_ROOT"
assert_eq "the default dir directly holds an executable scripts/stop.sh" \
    "$([ -f "$DEFAULT_DIR/scripts/stop.sh" ] && [ -x "$DEFAULT_DIR/scripts/stop.sh" ] && echo yes || echo no)" \
    "yes"

# A wrong RECLAIM_PROJECT_DIR is a precondition failure, named once, not a raw
# bash "No such file or directory" from inside the loop. Point it at a real
# directory holding no scripts/stop.sh, with a reclaimable engine present so the
# loop would otherwise reach stop.sh.
echo "preflight_reclaim: a project dir with no scripts/stop.sh fails as a precondition"
reset_host
add_proc 44011 coldrun /home/u/.lucidos/gateway/workspaces/coldrun
WRONG_DIR="$SANDBOX/wrong-root"
mkdir -p "$WRONG_DIR"
RECLAIM_PROJECT_DIR="$WRONG_DIR"
run_reclaim
RECLAIM_PROJECT_DIR="$FAKE_CHECKOUT"
assert_eq "exits non-zero" "$RC" "1"
assert_eq "stop.sh was never called" "$(stop_calls)" ""
assert_contains "the error names the missing stop.sh and its path" "$STDERR" \
    "no executable stop.sh at $WRONG_DIR/scripts/stop.sh"
assert_not_contains "not a raw bash no-such-file error" "$STDERR" "No such file or directory"

# ── the hermetic guarantee ──────────────────────────────────────────────
echo "preflight_reclaim: no real host call was made"
if [ -s "$BYPASS_LOG" ]; then
    fail "a real host call escaped the seams:"
    sed 's/^/        /' "$BYPASS_LOG"
else
    pass "no kill, pgrep, ps, pkill or killall reached the host"
fi

echo
echo "passed: $PASS   failed: $FAIL"
[ "$FAIL" -eq 0 ]
