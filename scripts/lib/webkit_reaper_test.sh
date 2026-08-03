#!/bin/bash
# Tests for scripts/lib/webkit_reaper.sh — the WebKit RSS reaper.
# Run: ./scripts/lib/webkit_reaper_test.sh   (no harness; direct, like e2e_test.sh)
#
# Exercises the reaper's SELECTION + KILL logic against REAL processes (so the
# kill is real and observable) but SYNTHETIC RSS/command rows (so over-cap vs
# under-cap is deterministic) — no browser required. The seam is
# `_reaper_list_processes`, overridden here to emit chosen "PID RSS_KB COMMAND"
# rows for sleeper PIDs we spawn.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SANDBOX="$(mktemp -d)"
SPAWNED=""
cleanup() {
    # shellcheck disable=SC2086
    [ -n "$SPAWNED" ] && kill $SPAWNED 2>/dev/null
    wait 2>/dev/null
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

# Isolate the pidfile to the sandbox so the lifecycle test never touches a real
# e2e workspace.
export E2E_WEBKIT_REAPER_PIDFILE="$SANDBOX/webkit-reaper.pid"

# shellcheck source=webkit_reaper.sh
source "$SCRIPT_DIR/webkit_reaper.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# ── seam override ──────────────────────────────────────────────────────
# The reaper only ever samples our synthetic rows. An empty SYNTHETIC_PS means
# "no candidates", NOT "scan the machine": this seam must never reach the real
# `ps`, because every row it emits is a real SIGKILL target.
#
# It used to fall back to the real `ps`, and on 2026-08-03 that killed two
# Claude Code sessions. A test that set SYNTHETIC_PS="" to assert "clean host
# returns 0" fed the whole host process table into the kill path, and this suite
# does not source ports.sh, so the is_protected_host_pid backstop was undefined
# and its `command -v` guard skipped it. Fail closed, matching the same seam in
# e2e_lock_test.sh.
SYNTHETIC_PS=""
_reaper_list_processes() {
    [ -n "$SYNTHETIC_PS" ] || return 0
    printf '%s\n' "$SYNTHETIC_PS"
}

# ── kill shim (ADR 0025; the ports_test.sh pattern) ────────────────────
# Running a scripts/lib test is in the same hazard class as a broad pkill, so
# the raw builtin may never send a lethal signal to a pid this test did not
# spawn. A blocked attempt is recorded and FAILS the suite at exit: a guard that
# silently swallowed the attempt would let the next selection bug read as green.
KILL_SHIM_LOG="$SANDBOX/blocked-kills.log"
: > "$KILL_SHIM_LOG"

# A pid is the test's own if spawn_sleeper created it, or if it descends from
# this script. The descendant arm covers the backgrounded reaper loop that
# start_webkit_reaper forks (and the `sleep` child inside it), so
# stop_webkit_reaper still works without the suite bookkeeping those pids by
# hand. Anything descended from this script is by definition something this
# script created.
_shim_is_ours() {
    local pid="$1" hops=0
    case " $SPAWNED " in *" $pid "*) return 0 ;; esac
    while [ -n "$pid" ] && [ "$pid" -gt 1 ] 2>/dev/null; do
        [ "$pid" = "$$" ] && return 0
        pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
        hops=$((hops + 1))
        [ "$hops" -gt 25 ] && break
    done
    return 1
}

kill() {
    local arg sig="" pids="" pid
    for arg in "$@"; do
        case "$arg" in
            -*) sig="$arg" ;;
            *)  pids="$pids $arg" ;;
        esac
    done
    # A liveness probe is harmless by definition, and the reaper cannot work
    # without it.
    if [ "$sig" = "-0" ]; then
        command kill "$@"
        return $?
    fi
    for pid in $pids; do
        if ! _shim_is_ours "$pid"; then
            printf '%s\n' "${sig:--TERM}:$pid" >> "$KILL_SHIM_LOG"
            echo "  webkit_reaper_test: BLOCKED lethal kill ${sig:--TERM} to pid $pid, which this test did not spawn" >&2
            return 1
        fi
    done
    command kill "$@"
}

# ── helpers ────────────────────────────────────────────────────────────
# Spawns a real sleeper and publishes its PID in SLEEPER_PID. Must run in the
# main shell (NOT command substitution): a backgrounded `sleep` started inside
# $(...) inherits the capture pipe and blocks the substitution until it exits.
# fds are redirected so the sleeper never holds a parent fd open.
SLEEPER_PID=""
spawn_sleeper() {
    sleep 600 >/dev/null 2>&1 &
    SLEEPER_PID=$!
    SPAWNED="$SPAWNED $SLEEPER_PID"
}

# A SIGKILL'd child becomes a zombie until reaped, and `kill -0` reports a zombie
# as still-existing — so treat "gone OR zombie" as dead.
is_dead() {
    local pid="$1"
    kill -0 "$pid" 2>/dev/null || return 0
    case "$(ps -o stat= -p "$pid" 2>/dev/null)" in
        Z*) return 0 ;;
        *) return 1 ;;
    esac
}

assert_dead() {
    local pid="$1" label="$2"
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        is_dead "$pid" && break
        sleep 0.1
    done
    if is_dead "$pid"; then
        pass "$label (pid $pid killed)"
        wait "$pid" 2>/dev/null   # reap the zombie (it is our child)
    else
        fail "$label (pid $pid still alive — should have been reaped)"
    fi
}

assert_alive() {
    local pid="$1" label="$2"
    if kill -0 "$pid" 2>/dev/null && ! is_dead "$pid"; then
        pass "$label (pid $pid left alone)"
    else
        fail "$label (pid $pid was killed — should have survived)"
    fi
}

WEBKIT_PATH="/Users/x/Library/Caches/ms-playwright/webkit-2287/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent"
CHROMIUM_PATH="/Users/x/Library/Caches/ms-playwright/chromium-1187/chrome-mac/Chromium.app/Contents/MacOS/Chromium"
SAFARI_PATH="/Applications/Safari.app/Contents/MacOS/Safari"
# A Claude Code process: argv[0] is the CC binary, and the matcher token appears
# far down its argv because the engine embeds the thread history into a huge
# --append-system-prompt. This is the exact shape that got SIGKILLed on
# 2026-08-03, so it stays as a fixture, not just as a comment.
CLAUDE_CODE_CMD="/Users/x/.local/bin/claude --output-format stream-json --append-system-prompt THREAD HISTORY: the leaked binary lives at ~/Library/Caches/ms-playwright/webkit-2287/com.apple.WebKit.GPU.xpc/Contents/MacOS/com.apple.WebKit.GPU.Development and reached 18.9 GB"

# ── Test 1: selection — kill the over-cap WebKit match, leave the rest ──
test_selection() {
    echo "test: kills over-cap WebKit child, leaves under-cap / non-matching processes"
    local over under safari chromium
    spawn_sleeper; over=$SLEEPER_PID
    spawn_sleeper; under=$SLEEPER_PID
    spawn_sleeper; safari=$SLEEPER_PID
    spawn_sleeper; chromium=$SLEEPER_PID

    # 7168 MB (>6144) WebKit, 51 MB (<6144) WebKit, 9216 MB Safari, 9216 MB chromium.
    SYNTHETIC_PS="$over 7340032 $WEBKIT_PATH
$under 52428 $WEBKIT_PATH
$safari 9437184 $SAFARI_PATH
$chromium 9437184 $CHROMIUM_PATH"

    E2E_WEBKIT_RSS_CAP_MB=6144 reap_once >/dev/null 2>&1

    assert_dead  "$over"     "over-cap WebKit WebContent"
    assert_alive "$under"    "under-cap WebKit WebContent"
    assert_alive "$safari"   "over-cap user Safari (not under Playwright cache)"
    assert_alive "$chromium" "over-cap Playwright chromium (webkit-only matcher)"

    SYNTHETIC_PS=""
}

# ── Test 2: cap is configurable ────────────────────────────────────────
test_cap_configurable() {
    echo "test: E2E_WEBKIT_RSS_CAP_MB raises/lowers the kill threshold"
    local hi lo
    spawn_sleeper; hi=$SLEEPER_PID   # 7168 MB process, but cap raised above it → survives
    spawn_sleeper; lo=$SLEEPER_PID   # 200 MB process, but cap lowered below it → killed

    SYNTHETIC_PS="$hi 7340032 $WEBKIT_PATH"
    E2E_WEBKIT_RSS_CAP_MB=8192 reap_once >/dev/null 2>&1
    assert_alive "$hi" "7168MB process under an 8192MB cap"

    SYNTHETIC_PS="$lo 204800 $WEBKIT_PATH"
    E2E_WEBKIT_RSS_CAP_MB=100 reap_once >/dev/null 2>&1
    assert_dead "$lo" "200MB process over a 100MB cap"

    SYNTHETIC_PS=""
}

# ── Test 3: match is configurable + default excludes others ────────────
test_match_override() {
    echo "test: E2E_WEBKIT_REAP_MATCH overrides the candidate substring"
    local tagged untagged
    spawn_sleeper; tagged=$SLEEPER_PID
    spawn_sleeper; untagged=$SLEEPER_PID

    # Custom token matches only the tagged command; the default-webkit row must
    # now be left alone because it lacks the custom token.
    SYNTHETIC_PS="$tagged 7340032 /tmp/custom-reap-token/some-binary
$untagged 7340032 $WEBKIT_PATH"

    E2E_WEBKIT_RSS_CAP_MB=6144 E2E_WEBKIT_REAP_MATCH="custom-reap-token" \
        reap_once >/dev/null 2>&1

    assert_dead  "$tagged"   "process matching custom token"
    assert_alive "$untagged" "WebKit process NOT matching custom token"

    SYNTHETIC_PS=""
}

# ── Test 3b: mentioning the path is not being the process ──────────────
# The 2026-08-03 regression. The matcher is tested against argv[0], so a process
# that merely QUOTES the browsers-cache path somewhere in its arguments must
# never be a candidate, however far over the cap it is. Without this the reaper
# SIGKILLs the Claude Code session that is reading this file.
test_argv0_only_never_matches_a_mention() {
    echo "test: a process that only MENTIONS the browsers path is never a candidate"
    local cc browser
    spawn_sleeper; cc=$SLEEPER_PID
    spawn_sleeper; browser=$SLEEPER_PID

    # Both wildly over the cap. Only the one actually RUNNING a browser binary
    # may be reaped.
    SYNTHETIC_PS="$cc 9437184 $CLAUDE_CODE_CMD
$browser 9437184 $WEBKIT_PATH --inspector-pipe"

    E2E_WEBKIT_RSS_CAP_MB=6144 reap_once >/dev/null 2>&1

    assert_alive "$cc"      "Claude Code process quoting the path in its argv"
    assert_dead  "$browser" "real WebKit child (argv[0] under the cache)"

    SYNTHETIC_PS=""
}

# ── Test 4: never kill init or our own shell ───────────────────────────
test_skips_self_and_init() {
    echo "test: PID<=1 and the script's own PID are never killed"
    # Contrive matching, wildly-over-cap rows for PID 1 and $$ (this shell).
    SYNTHETIC_PS="1 99999999 $WEBKIT_PATH
$$ 99999999 $WEBKIT_PATH"

    # If the guard were missing this would SIGKILL the test runner itself.
    E2E_WEBKIT_RSS_CAP_MB=6144 reap_once >/dev/null 2>&1
    local rc=$?

    if [ "$rc" = "0" ] && kill -0 "$$" 2>/dev/null; then
        pass "self ($$) and init survived a matching over-cap row"
    else
        fail "reap_once touched a protected PID (rc=$rc)"
    fi

    SYNTHETIC_PS=""
}

# ── Test 4b: never kill the reaper's own loop (pidfile-resolved) ───────
# Inside the backgrounded loop, WEBKIT_REAPER_PID is empty, so reap_once must
# fall back to the pidfile to learn its own PID. Simulate that: pidfile points at
# a sleeper, and a matching over-cap row names that same PID — it must survive.
test_skips_reaper_own_pid() {
    echo "test: the reaper never kills its own loop (PID resolved from pidfile)"
    local me
    spawn_sleeper; me=$SLEEPER_PID
    echo "$me" > "$E2E_WEBKIT_REAPER_PIDFILE"
    WEBKIT_REAPER_PID=""   # force the pidfile-fallback path

    SYNTHETIC_PS="$me 7340032 $WEBKIT_PATH"
    E2E_WEBKIT_RSS_CAP_MB=6144 reap_once >/dev/null 2>&1

    assert_alive "$me" "reaper loop PID (from pidfile)"

    rm -f "$E2E_WEBKIT_REAPER_PIDFILE"
    SYNTHETIC_PS=""
}

# ── Test 4c: interval validation rejects busy-loop values ──────────────
test_interval_validation() {
    echo "test: _reaper_interval_s rejects non-numeric / zero, keeps valid"
    local got
    got=$(E2E_WEBKIT_REAP_INTERVAL_S=5s _reaper_interval_s)
    if [ "$got" = "5" ]; then pass "non-numeric '5s' → default 5"; else fail "got '$got' (expected 5)"; fi
    got=$(E2E_WEBKIT_REAP_INTERVAL_S=0 _reaper_interval_s)
    if [ "$got" = "5" ]; then pass "zero → default 5 (no busy-loop)"; else fail "got '$got' (expected 5)"; fi
    got=$(E2E_WEBKIT_REAP_INTERVAL_S=3 _reaper_interval_s)
    if [ "$got" = "3" ]; then pass "valid '3' kept"; else fail "got '$got' (expected 3)"; fi
}

# ── Test 5: default match resolution ───────────────────────────────────
test_default_match() {
    echo "test: _reaper_match default + PLAYWRIGHT_BROWSERS_PATH override"
    local got
    got=$(unset E2E_WEBKIT_REAP_MATCH PLAYWRIGHT_BROWSERS_PATH; _reaper_match)
    if [ "$got" = "ms-playwright/webkit" ]; then
        pass "default match = $got"
    else
        fail "expected 'ms-playwright/webkit', got '$got'"
    fi

    got=$(unset E2E_WEBKIT_REAP_MATCH; PLAYWRIGHT_BROWSERS_PATH=/opt/pw _reaper_match)
    if [ "$got" = "/opt/pw/webkit" ]; then
        pass "PLAYWRIGHT_BROWSERS_PATH override = $got"
    else
        fail "expected '/opt/pw/webkit', got '$got'"
    fi
}

# ── Test 6: start/stop lifecycle leaves no loop behind ─────────────────
test_start_stop_lifecycle() {
    echo "test: start_webkit_reaper / stop_webkit_reaper lifecycle"
    # Feed a harmless non-matching row so the live loop never kills anything real.
    SYNTHETIC_PS="99999 100 /bin/true"

    E2E_WEBKIT_REAP_INTERVAL_S=1 E2E_WEBKIT_RSS_CAP_MB=6144 \
        start_webkit_reaper >/dev/null 2>&1

    if [ -n "${WEBKIT_REAPER_PID:-}" ] && kill -0 "$WEBKIT_REAPER_PID" 2>/dev/null; then
        pass "reaper loop started (pid $WEBKIT_REAPER_PID)"
    else
        fail "reaper loop did not start"
    fi

    if [ -f "$E2E_WEBKIT_REAPER_PIDFILE" ]; then
        pass "pidfile written"
    else
        fail "pidfile not written"
    fi

    local loop_pid="${WEBKIT_REAPER_PID:-}"
    stop_webkit_reaper >/dev/null 2>&1

    # stop_webkit_reaper SIGTERMs the loop, and the loop is disowned, so the
    # `wait` inside it returns immediately rather than reaping. Signal delivery,
    # the loop's TERM trap and its exit are all asynchronous, so poll for the
    # exit the same bounded way assert_dead does instead of racing it.
    local _ stopped=0
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        [ -n "$loop_pid" ] || break
        if ! kill -0 "$loop_pid" 2>/dev/null || is_dead "$loop_pid"; then
            stopped=1
            break
        fi
        sleep 0.1
    done
    if [ -n "$loop_pid" ] && [ "$stopped" != "1" ]; then
        fail "reaper loop $loop_pid still alive after stop"
    else
        pass "reaper loop stopped"
    fi

    if [ -f "$E2E_WEBKIT_REAPER_PIDFILE" ]; then
        fail "pidfile not removed on stop"
    else
        pass "pidfile removed on stop"
    fi

    # Idempotent: a second stop with nothing running must succeed quietly.
    if stop_webkit_reaper >/dev/null 2>&1; then
        pass "stop_webkit_reaper is idempotent (no-op when not running)"
    else
        fail "stop_webkit_reaper errored when nothing was running"
    fi

    SYNTHETIC_PS=""
}

# ── Test 7: disabled via env ───────────────────────────────────────────
test_disable_knob() {
    echo "test: E2E_WEBKIT_REAP=0 disables the reaper"
    WEBKIT_REAPER_PID=""
    E2E_WEBKIT_REAP=0 start_webkit_reaper >/dev/null 2>&1
    if [ -z "${WEBKIT_REAPER_PID:-}" ]; then
        pass "no loop spawned when disabled"
    else
        fail "loop spawned despite E2E_WEBKIT_REAP=0 (pid ${WEBKIT_REAPER_PID})"
        stop_webkit_reaper >/dev/null 2>&1
    fi
}

# ── Test 8: a whitespace match token disarms the guard, loudly ─────────
test_whitespace_match_warns() {
    echo "test: start_webkit_reaper warns when the match token contains whitespace"
    local out_file="$SANDBOX/whitespace-warning.txt"
    WEBKIT_REAPER_PID=""
    SYNTHETIC_PS="99999 100 /bin/true"
    # Redirect to a file rather than capturing with $(...): start_webkit_reaper
    # backgrounds the loop, and a backgrounded child inherits the capture pipe,
    # so the substitution would block until the loop exits (it never does).
    E2E_WEBKIT_REAP_INTERVAL_S=1 E2E_WEBKIT_REAP_MATCH="ms playwright/webkit" \
        start_webkit_reaper > "$out_file" 2>&1
    stop_webkit_reaper >/dev/null 2>&1

    if grep -q "WARNING.*whitespace" "$out_file"; then
        pass "whitespace token warns that the guard is off"
    else
        fail "no warning for a whitespace match token (got: $(cat "$out_file"))"
    fi

    SYNTHETIC_PS=""
}

test_selection
test_cap_configurable
test_match_override
test_argv0_only_never_matches_a_mention
test_skips_self_and_init
test_skips_reaper_own_pid
test_interval_validation
test_default_match
test_start_stop_lifecycle
test_disable_knob
test_whitespace_match_warns

# The kill shim must have blocked nothing. A test that tried to signal a process
# it did not spawn is a hard failure, not a warning: that is precisely how this
# suite SIGKILLed two Claude Code sessions on 2026-08-03.
if [ -s "$KILL_SHIM_LOG" ]; then
    echo ""
    echo "  FAIL: the suite attempted a lethal signal to a pid it did not spawn:"
    sed 's/^/         /' "$KILL_SHIM_LOG"
    FAIL=$((FAIL + 1))
else
    pass "no lethal signal was attempted against a foreign process"
fi

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
