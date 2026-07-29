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
# When SYNTHETIC_PS is set, the reaper samples our synthetic rows; otherwise it
# falls back to the real `ps` so unconverted tests still work.
SYNTHETIC_PS=""
_reaper_list_processes() {
    if [ -n "$SYNTHETIC_PS" ]; then
        printf '%s\n' "$SYNTHETIC_PS"
    else
        ps -Aww -o pid=,rss=,command= 2>/dev/null
    fi
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

    if [ -n "$loop_pid" ] && kill -0 "$loop_pid" 2>/dev/null; then
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

test_selection
test_cap_configurable
test_match_override
test_skips_self_and_init
test_skips_reaper_own_pid
test_interval_validation
test_default_match
test_start_stop_lifecycle
test_disable_knob

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
