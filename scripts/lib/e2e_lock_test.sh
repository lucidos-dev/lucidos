#!/bin/bash
# Tests for scripts/lib/e2e_lock.sh — the single-writer lock on the shared
# e2e-test workspace that prevents concurrent CC sessions (and re-spawned nightly
# runs) from clobbering each other.
#
# Two failure modes are covered:
#   1. CONCURRENT runs — Playwright multiplies WebKit GPU processes; two parallel
#      runs on a 32 GB Mac OOM'd the system on 2026-04-19. A live-PID lock must
#      hard-fail the second entrant.
#   2. ORPHAN PILE-UP — an interrupted run leaves orphaned browsers + engine
#      alive; the nightly orchestrator re-spawned the suite three times on
#      2026-06-21, each re-spawn reclaiming the "free" stale lock and stacking a
#      fresh set of browsers on top of the orphans → 23.5 GB compressed + 14 GB
#      swap. Reclaiming a stale lock must SWEEP the orphans first (or refuse).
#
# Hermetic: fake orphans are real `sleep` sleepers fed through a synthetic
# `_e2e_orphan_ps` / overridden `_e2e_list_orphans` — no browser is ever spawned,
# and $E2E_WORKSPACE is pinned to a sandbox so the real e2e workspace is untouched.
#
# Run: ./scripts/lib/e2e_lock_test.sh   (no harness; direct, like webkit_reaper_test.sh)
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Isolate test runs from any real e2e workspace lock AND workspace. Pinning
# E2E_WORKSPACE to the sandbox is load-bearing: the real `_e2e_list_orphans`
# reads <E2E_WORKSPACE>/.lucidos/engine.pid, so without this the test could
# detect — and SIGUSR1 — a real running e2e engine.
TMPROOT="$(mktemp -d -t lucidos-e2e-lock-test.XXXXXX)"
export E2E_LOCK_DIR_OVERRIDE="$TMPROOT/lock-dir"
mkdir -p "$E2E_LOCK_DIR_OVERRIDE"
export E2E_WORKSPACE="$TMPROOT/ws"
mkdir -p "$E2E_WORKSPACE/.lucidos"
OUT_DIR="$TMPROOT/out"
mkdir -p "$OUT_DIR"

SPAWNED=""
cleanup() {
    # shellcheck disable=SC2086
    [ -n "$SPAWNED" ] && kill -KILL $SPAWNED 2>/dev/null
    wait 2>/dev/null
    rm -rf "$TMPROOT"
}
trap cleanup EXIT

# ── neutralize the lucidos CLI ──────────────────────────────────────────
# A recording stub shadows the real CLI for the WHOLE file, so even a regression
# in the emit guard cannot reach a live engine: the worst it could do is append a
# line here. Same posture as the ps and cwd seams below, and as the `kill` shim
# in ports_test.sh (ADR 0025).
#
# One line per call, tab separated. No argument the lock library passes can
# contain a tab or a newline: `_e2e_json_escape` strips control characters from
# every interpolated value, and the summary is the library's own text. Two
# trailing fields record what the ARGUMENTS cannot say. The workspace the call
# was addressed to (`WS=<path>`, or `WS_UNSET`), because that is not an argument
# at all: `lucidos events emit` reads it from the environment, and the entry
# scripts repoint it between a hold's acquire and its release. And whether the
# lock file existed AT CALL TIME, which is how the ordering invariant (announce
# a release only once the file is gone) is asserted rather than assumed.
STUB_DIR="$TMPROOT/stub"
mkdir -p "$STUB_DIR"
CAPTURE="$TMPROOT/lucidos-calls.txt"
: > "$CAPTURE"
export CAPTURE
EMIT_LOCK="$E2E_WORKSPACE/.lucidos/e2e.lock"
STUB_WATCH_LOCK="$EMIT_LOCK"
export STUB_WATCH_LOCK
cat > "$STUB_DIR/lucidos" <<'STUB'
#!/bin/bash
# Delay ONE event type before it records, which is what makes the ordering test
# deterministic instead of a coin flip: argv is `events emit <Type> ...`, so $3
# names the event.
if [ "${3:-}" = "${STUB_PREDELAY_EVENT:-}" ] && [ -n "${STUB_PREDELAY_S:-}" ]; then
    sleep "$STUB_PREDELAY_S"
fi
{
    printf '%s\t' "$@"
    if [ -n "${LUCIDOS_WORKSPACE+set}" ]; then
        printf 'WS=%s\t' "$LUCIDOS_WORKSPACE"
    else
        printf 'WS_UNSET\t'
    fi
    if [ -e "${STUB_WATCH_LOCK:-/nonexistent}" ]; then
        printf 'LOCKFILE_PRESENT'
    else
        printf 'LOCKFILE_ABSENT'
    fi
    printf '\n'
} >> "$CAPTURE"
# Stand in for a wedged engine. Recorded BEFORE sleeping, so the hang test can
# still assert the call was made.
[ -n "${STUB_SLEEP_S:-}" ] && sleep "$STUB_SLEEP_S"
exit "${STUB_EXIT:-0}"
STUB
chmod +x "$STUB_DIR/lucidos"
PATH="$STUB_DIR:$PATH"
export PATH

# The stub's knobs, and the library's emit bound, are EXPORTED ONCE here with
# their inert defaults so each case can set them with a plain assignment inside
# its subshell. Exporting them in the subshells instead is what shellcheck's
# SC2030/SC2031 complain about, correctly in general (a value set in one subshell
# is invisible to the next) even though each case here wants exactly that.
# Declared empty: the stub reads `${STUB_SLEEP_S:-}` / `${STUB_EXIT:-0}` and the
# library `${E2E_LOCK_EMIT_TIMEOUT_S:-5}`, so empty is the default in all three.
export STUB_SLEEP_S=""
export STUB_EXIT=""
export E2E_LOCK_EMIT_TIMEOUT_S=""
export STUB_PREDELAY_EVENT=""
export STUB_PREDELAY_S=""

# PINNED, because the suite is normally run from inside a coding-agent session
# where the engine has already set this, and `_e2e_stand_down_lock_waits` keys
# on it: without a pin, the stand-down cases would pass for whoever ran them in
# a session and silently no-op for anyone running the file by hand. The value
# reaches the CLI stub only, never an engine, since $E2E_LOCK_DIR_OVERRIDE and
# the stub each block that independently.
export LUCIDOS_THREAD_ID="11111111-2222-3333-4444-555555555555"

TAB="$(printf '\t')"

# The first recorded call for an event type, and its three interesting fields.
# Counted from the END because the stub appends its two observations after the
# arguments, so a new argument in the middle cannot shift them.
emit_call() { grep "^events${TAB}emit${TAB}$1${TAB}" "$CAPTURE" | head -1; }
emit_payload() { emit_call "$1" | awk -F'\t' '{ print $(NF - 2) }'; }
emit_workspace() { emit_call "$1" | awk -F'\t' '{ print $(NF - 1) }'; }
emit_marker() { emit_call "$1" | awk -F'\t' '{ print $NF }'; }

# The stand-down `acquire_e2e_lock` makes once it holds the lock, and the
# workspace it was addressed to. A different subcommand from the emits, so the
# two never match each other's greps.
standdown_call() { grep "^event-waits${TAB}cancel${TAB}" "$CAPTURE" | head -1; }
standdown_workspace() { standdown_call | awk -F'\t' '{ print $(NF - 1) }'; }

# Wait for an event to be recorded, bounded. Needed only by the cases that
# deliberately do NOT wait for the announcement acquire_e2e_lock backgrounds:
# once their subshell exits, that child is re-parented and the test shell can no
# longer `wait` for it, so its write would otherwise land in $CAPTURE after the
# NEXT case has truncated it and be read as that case's event.
drain_emit() {
    local event="$1" waited=0
    while [ -z "$(emit_call "$event")" ] && [ "$waited" -lt 50 ]; do
        sleep 0.1
        waited=$((waited + 1))
    done
}

# The same, for the stand-down, which shares that child and is therefore
# re-parented the same way.
drain_standdown() {
    local waited=0
    while [ -z "$(standdown_call)" ] && [ "$waited" -lt 50 ]; do
        sleep 0.1
        waited=$((waited + 1))
    done
}

# Start an emit case from a known state. The emit cases run with
# $E2E_LOCK_DIR_OVERRIDE unset (that variable IS the guard), so their lock lands
# at <E2E_WORKSPACE>/.lucidos/e2e.lock instead, which this file already pins to
# the sandbox.
reset_emit_sandbox() {
    rm -f "$EMIT_LOCK" "$E2E_WORKSPACE/.lucidos/engine.pid"
    E2E_LOCK_OWNED=""
    SYNTHETIC_PS=""
    : > "$CAPTURE"
}

source "$PROJECT_DIR/scripts/lib/e2e_lock.sh"

# ── neutralize the real process scan ────────────────────────────────────
# Override the ps seam so the real `_e2e_list_orphans` runs against synthetic
# rows, never the real Playwright processes that might be running on this host
# during a nightly. SYNTHETIC_PS holds "PID COMMAND" rows (empty → no orphans).
# We filter by liveness so the feed mirrors real `ps`: a SIGKILL'd sleeper
# becomes a zombie that `ps` would drop — which is what makes a post-sweep
# re-scan return empty and lets the reclaim proceed. `orphan_alive` is defined
# below; it exists by the time any test calls this at runtime.
SYNTHETIC_PS=""
_e2e_orphan_ps() {
    [ -n "$SYNTHETIC_PS" ] || return 0
    local pid rest
    while read -r pid rest; do
        [ -z "$pid" ] && continue
        orphan_alive "$pid" && printf '%s %s\n' "$pid" "$rest"
    done <<EOF
$SYNTHETIC_PS
EOF
    return 0
}

# ── neutralize the real cwd lookup ──────────────────────────────────────
# The `agent` kind asks the kernel for a pid's cwd, which for a sleeper is this
# test's own directory and would never match. SYNTHETIC_CWD holds "PID PATH"
# rows so a fixture can claim to be running inside (or outside) the sandbox
# workspace. Fails CLOSED like the ps seam above: an unlisted pid answers empty,
# never a real lookup, so a synthetic fixture can never resolve a host process.
SYNTHETIC_CWD=""
_e2e_proc_cwd() {
    local want="$1" pid path
    while read -r pid path; do
        [ -z "$pid" ] && continue
        [ "$pid" = "$want" ] && { printf '%s\n' "$path"; return 0; }
    done <<EOF
$SYNTHETIC_CWD
EOF
    return 0
}

PASS=0
FAIL=0

assert_eq() {
    local expected="$1" actual="$2" msg="$3"
    if [ "$expected" = "$actual" ]; then
        echo "  PASS: $msg"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $msg (expected '$expected', got '$actual')"
        FAIL=$((FAIL + 1))
    fi
}

pass() { echo "  PASS: $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

reset_lock_dir() {
    rm -rf "$E2E_LOCK_DIR_OVERRIDE"
    mkdir -p "$E2E_LOCK_DIR_OVERRIDE"
    # Each test starts with a fresh lock owner and no leftover engine pidfile.
    E2E_LOCK_OWNED=""
    rm -f "$E2E_WORKSPACE/.lucidos/engine.pid"
    SYNTHETIC_PS=""
}

# Spawn a real sleeper and publish its PID in SLEEPER_PID. Must run in the main
# shell (not command substitution): a backgrounded `sleep` inside $(...) inherits
# the capture pipe and blocks the substitution until it exits.
SLEEPER_PID=""
spawn_sleeper() {
    sleep 600 >/dev/null 2>&1 &
    SLEEPER_PID=$!
    SPAWNED="$SPAWNED $SLEEPER_PID"
}

# A sleeper that IGNORES SIGTERM, standing in for a coding agent wedged past the
# polite ask. The agent reap escalates to SIGKILL for exactly this case.
spawn_stubborn_sleeper() {
    # A LOOP, not a bare `sleep`: bash exec-optimizes a single trailing command
    # and the trap goes with it, so `trap "" TERM; sleep 600` dies on SIGTERM
    # and never exercises the escalation this fixture exists for.
    #
    # And the caller must WAIT for the trap to be installed. `&` returns before
    # the child has run a single line, so a reap firing immediately catches it
    # with the default disposition and it dies on the polite SIGTERM: the test
    # then passes (the process is gone) while proving nothing. The readiness
    # file is what makes the escalation the only path that can clear it.
    local ready="$TMPROOT/stubborn-ready.$$"
    rm -f "$ready"
    bash -c 'trap "" TERM; : > "$1"; while :; do sleep 1; done' _ "$ready" >/dev/null 2>&1 &
    SLEEPER_PID=$!
    SPAWNED="$SPAWNED $SLEEPER_PID"
    local waited=0
    while [ ! -e "$ready" ] && [ "$waited" -lt 100 ]; do
        sleep 0.05
        waited=$((waited + 1))
    done
    [ -e "$ready" ] || fail "stubborn sleeper never installed its SIGTERM trap"
    rm -f "$ready"
}

# A SIGKILL'd child becomes a zombie until reaped, and `kill -0` reports a zombie
# as still-existing — so treat "gone OR zombie" as dead. This mirrors production,
# where a reaped orphan (reparented to init) is truly gone after SIGKILL.
# Wait (bounded, ~3s) for a signalled pid to actually be gone. Signal delivery
# and reaping are asynchronous, so an assertion fired the instant after a kill
# reads a live process and fails for a reason that has nothing to do with the
# code under test.
wait_until_dead() {
    local pid="$1" waited=0
    while orphan_alive "$pid" && [ "$waited" -lt 60 ]; do
        sleep 0.05
        waited=$((waited + 1))
    done
}

orphan_alive() {
    local pid="$1"
    kill -0 "$pid" 2>/dev/null || return 1
    case "$(ps -o stat= -p "$pid" 2>/dev/null)" in
        Z*) return 1 ;;
        *) return 0 ;;
    esac
}

# ── 1. First acquire succeeds ────────────────────────────────────────────
echo "Test 1: first acquire succeeds"
reset_lock_dir
acquire_e2e_lock e2e-browser >"$OUT_DIR"/test-1.out 2>&1
assert_eq "0" "$?" "first acquire returns 0"
assert_eq "1" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "lock file created"
release_e2e_lock

# ── 2. Second concurrent acquire fails (live-PID conflict) ───────────────
echo "Test 2: concurrent acquire blocked"
reset_lock_dir
acquire_e2e_lock e2e-browser >"$OUT_DIR"/test-2a.out 2>&1
first_rc=$?
acquire_e2e_lock e2e-api >"$OUT_DIR"/test-2b.out 2>&1
second_rc=$?
assert_eq "0" "$first_rc" "first acquire returns 0"
assert_eq "1" "$second_rc" "second acquire returns 1"
if grep -q "another e2e run is in progress" "$OUT_DIR"/test-2b.out; then
    pass "error message names the conflict"
else
    fail "error message missing 'another e2e run is in progress'"
    echo "  ---"; cat "$OUT_DIR"/test-2b.out; echo "  ---"
fi
if grep -qE "PID [0-9]+" "$OUT_DIR"/test-2b.out; then
    pass "error message includes owning PID"
else
    fail "error message missing owning PID"
fi
release_e2e_lock

# ── 3. Stale lock (dead PID), NO orphans → reclaimed ─────────────────────
echo "Test 3: stale lock with no orphans is reclaimed"
reset_lock_dir
# 999999 is virtually guaranteed not to exist (macOS PID_MAX is 99999).
cat > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" <<EOF
PID=999999
THREAD_ID=ghost
WORKTREE=/tmp/ghost
STARTED=2020-01-01T00:00:00Z
SCRIPT=e2e-browser
EOF
acquire_e2e_lock e2e-browser >"$OUT_DIR"/test-3.out 2>&1
assert_eq "0" "$?" "clean stale lock reclaimed (acquire returns 0)"
# A clean reclaim must not print a sweep line — there were no orphans.
if grep -q "sweeping before" "$OUT_DIR"/test-3.out; then
    fail "clean stale reclaim wrongly ran an orphan sweep"
    echo "  ---"; cat "$OUT_DIR"/test-3.out; echo "  ---"
else
    pass "clean stale reclaim did not run a sweep"
fi
release_e2e_lock

# ── 4. Stale lock + orphan present → reaped, THEN reclaimed ───────────────
# The crux of the 2026-06-21 fix: a dead-PID lock with a live orphan must NOT be
# blindly reclaimed. A REAL sleeper plays the orphaned Playwright browser (fed
# through the ps seam with an ms-playwright path). The default reaper SIGKILLs it;
# the liveness-aware ps feed then drops it, so the lock is reclaimed.
echo "Test 4: stale lock with a live orphan is swept then reclaimed"
reset_lock_dir
spawn_sleeper
orphan=$SLEEPER_PID
SYNTHETIC_PS="$orphan /Users/x/Library/Caches/ms-playwright/webkit-2287/WebContent.app/Contents/MacOS/WebContent"
cat > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" <<EOF
PID=999999
THREAD_ID=ghost
WORKTREE=/tmp/ghost
STARTED=2020-01-01T00:00:00Z
SCRIPT=e2e-browser
EOF
acquire_e2e_lock e2e-browser >"$OUT_DIR"/test-4.out 2>&1
rc4=$?
SYNTHETIC_PS=""
assert_eq "0" "$rc4" "acquire succeeds after sweeping the orphan"
# The orphan must actually be dead — the sweep was real, not a no-op.
if orphan_alive "$orphan"; then
    fail "orphan pid $orphan still alive — sweep did not actually reap it"
else
    pass "orphan pid $orphan was reaped (real SIGKILL)"
fi
if grep -q "reaped orphan browser pid=$orphan" "$OUT_DIR"/test-4.out; then
    pass "sweep logged the reap (deliberate, not silent)"
else
    fail "sweep did not log the reap"
    echo "  ---"; cat "$OUT_DIR"/test-4.out; echo "  ---"
fi
assert_eq "1" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "lock reacquired (file present)"
wait "$orphan" 2>/dev/null
release_e2e_lock

# ── 5. Stale lock + orphan that survives the sweep → REFUSE ───────────────
# When the sweep cannot clear the orphan, acquire must REFUSE (return 1) and NOT
# stack a fresh run. Simulated by a no-op reaper over a live orphan.
echo "Test 5: stale lock with an unreapable orphan is refused (not stacked)"
reset_lock_dir
spawn_sleeper
orphan=$SLEEPER_PID
SYNTHETIC_PS="$orphan /Users/x/Library/Caches/ms-playwright/webkit-2287/WebContent.app/Contents/MacOS/WebContent"
cat > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" <<EOF
PID=999999
THREAD_ID=ghost
WORKTREE=/tmp/ghost
STARTED=2020-01-01T00:00:00Z
SCRIPT=e2e-browser
EOF
# Simulate "couldn't kill it": stub the reaper to a no-op, saving the real one so
# later tests still have it (bash can't restore a shadowed function via unset).
real_reap="$(declare -f _e2e_reap_orphans)"
_e2e_reap_orphans() { :; }
E2E_ORPHAN_REAP_TIMEOUT_S=1 acquire_e2e_lock e2e-browser >"$OUT_DIR"/test-5.out 2>&1
rc5=$?
eval "$real_reap"   # restore the real reaper
SYNTHETIC_PS=""
assert_eq "1" "$rc5" "acquire refuses when the orphan survives the sweep"
if grep -q "orphaned processes that the" "$OUT_DIR"/test-5.out; then
    pass "refusal message explains the orphan pile-up"
else
    fail "refusal message missing"
    echo "  ---"; cat "$OUT_DIR"/test-5.out; echo "  ---"
fi
if grep -q "browser $orphan" "$OUT_DIR"/test-5.out; then
    pass "refusal names the surviving orphan PID"
else
    fail "refusal does not name the surviving orphan"
fi
# We refused — the stale lock must be left intact (not reclaimed) and we own nothing.
assert_eq "1" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "stale lock left intact on refusal"
assert_eq "999999" "$(sed -n 's/^PID=//p' "$E2E_LOCK_DIR_OVERRIDE/e2e.lock")" "stale lock NOT overwritten"
assert_eq "" "$E2E_LOCK_OWNED" "we do not own the lock after refusal"
if orphan_alive "$orphan"; then
    pass "we refused rather than proceeding past a live orphan"
else
    fail "orphan died unexpectedly (no-op reaper should have left it alive)"
fi
kill -KILL "$orphan" 2>/dev/null; wait "$orphan" 2>/dev/null

# ── 6. Orphan scan: matches Playwright browsers + engine, ignores others ─
# The scan must key off the ms-playwright cache path (browsers) and the e2e
# workspace's own engine.pid (engine) — never the user's Safari or anything else.
echo "Test 6: orphan scan matches Playwright + engine pidfile, ignores user Safari"
reset_lock_dir
spawn_sleeper; wk=$SLEEPER_PID       # Playwright WebKit child
spawn_sleeper; sf=$SLEEPER_PID       # user's own Safari
spawn_sleeper; eng=$SLEEPER_PID      # e2e workspace engine (via pidfile)
spawn_sleeper; cc=$SLEEPER_PID       # a Claude Code session that only MENTIONS the path
# The cc row is the 2026-08-03 regression. Tokens are matched against argv[0], so
# a process whose argv merely QUOTES the browsers-cache path is not a browser.
# Claude Code carries the engine's thread history inside a huge
# --append-system-prompt, and this branch SIGKILLs with no RSS threshold at all,
# so a full-command-line match here kills the session that runs the suite.
SYNTHETIC_PS="$wk /Users/x/Library/Caches/ms-playwright/webkit-2287/WebContent.app/Contents/MacOS/WebContent
$sf /Applications/Safari.app/Contents/MacOS/Safari
$cc /Users/x/.local/bin/claude --append-system-prompt THREAD HISTORY: the leak lives under ms-playwright/webkit-2287/ and ms-playwright/chromium-1187/"
echo "$eng" > "$E2E_WORKSPACE/.lucidos/engine.pid"
scan_out="$(_e2e_list_orphans)"
SYNTHETIC_PS=""
if printf '%s\n' "$scan_out" | grep -qx "browser $wk"; then
    pass "Playwright WebKit child detected as a browser orphan"
else
    fail "Playwright WebKit child not detected (scan: $scan_out)"
fi
if printf '%s\n' "$scan_out" | grep -q "$sf"; then
    fail "user Safari wrongly flagged as an orphan (scan: $scan_out)"
else
    pass "user Safari correctly ignored (not under ms-playwright)"
fi
if printf '%s\n' "$scan_out" | grep -qx "engine $eng"; then
    pass "e2e workspace engine detected via engine.pid"
else
    fail "e2e engine not detected via pidfile (scan: $scan_out)"
fi
if printf '%s\n' "$scan_out" | grep -q "$cc"; then
    fail "Claude Code process wrongly flagged: argv MENTIONS the path, argv[0] does not (scan: $scan_out)"
else
    pass "process that only mentions the browsers path correctly ignored"
fi
rm -f "$E2E_WORKSPACE/.lucidos/engine.pid"
kill -KILL "$wk" "$sf" "$eng" "$cc" 2>/dev/null; wait 2>/dev/null

# ── 6b. A whitespace browsers path disarms the scan, loudly ──────────────
# argv[0] is read up to the first space, so a PLAYWRIGHT_BROWSERS_PATH with a
# space in it can never match. That is a blind orphan scan, which is exactly the
# pile-up this lock prevents, so it must never happen silently.
echo "Test 6b: whitespace browsers path warns that orphan detection is off"
SYNTHETIC_PS=""
PLAYWRIGHT_BROWSERS_PATH="/tmp/My Browsers/ms-playwright" \
    _e2e_list_orphans > "$OUT_DIR"/test-6b.out 2> "$OUT_DIR"/test-6b.err
if grep -q "WARNING.*whitespace.*OFF" "$OUT_DIR"/test-6b.err; then
    pass "whitespace browsers path warns that orphan detection is off"
else
    fail "no warning for a whitespace browsers path (stderr: $(cat "$OUT_DIR"/test-6b.err))"
fi
if [ -s "$OUT_DIR"/test-6b.out ]; then
    fail "warning leaked into the scan's stdout (would be parsed as an orphan)"
else
    pass "warning goes to stderr, leaving the scan's stdout parseable"
fi

# ── 6c. Coding-agent orphans are found by CWD, never by argv ─────────────
# The 2026-08-07 gap: the suite's own tests spawn Claude Code subprocesses, the
# engine dies, they are re-parented to init, and nothing looked for them. Four
# survived 55 minutes and drove the host into memory exhaustion.
#
# The discriminator has to be the cwd. argv[0] is the same `.../bin/claude` for
# the user's own sessions, and the rest of the command line is worse than
# useless: a coding agent carries the thread history in a ~22 KB
# --append-system-prompt, so a session DISCUSSING this workspace quotes its
# paths verbatim. That is the 2026-08-03 kill, reproduced here as the `mention`
# fixture.
echo "Test 6c: coding-agent orphans matched on cwd, not on argv"
reset_lock_dir
spawn_sleeper; inside=$SLEEPER_PID     # e2e-spawned agent: cwd in the workspace
spawn_sleeper; outside=$SLEEPER_PID    # the user's own session, elsewhere
spawn_sleeper; mention=$SLEEPER_PID    # a session that only QUOTES the path
spawn_sleeper; eng2=$SLEEPER_PID       # the workspace engine: has its own kind
SYNTHETIC_PS="$inside /Users/x/.local/bin/claude --settings /whatever/cc-settings.json
$outside /Users/x/.local/bin/claude --settings /Users/x/other/cc-settings.json
$mention /Users/x/.local/bin/claude --append-system-prompt THREAD HISTORY: orphans under $E2E_WORKSPACE/.lucidos/worktrees/ survived 55 minutes
$eng2 /Users/x/.launch/release/e2e-test-hooks/lucidos-engine"
SYNTHETIC_CWD="$inside $E2E_WORKSPACE/.lucidos/worktrees/thread-abc
$outside /Users/x/workspaces/dev/.lucidos/worktrees/thread-def
$mention /Users/x/workspaces/dev/.lucidos/worktrees/thread-ghi
$eng2 $E2E_WORKSPACE"
scan_out="$(_e2e_list_orphans)"
SYNTHETIC_PS=""; SYNTHETIC_CWD=""
if printf '%s\n' "$scan_out" | grep -qx "agent $inside"; then
    pass "agent with cwd inside the e2e workspace detected"
else
    fail "e2e-spawned agent not detected (scan: $scan_out)"
fi
if printf '%s\n' "$scan_out" | grep -q "$outside"; then
    fail "the user's own coding-agent session wrongly flagged (scan: $scan_out)"
else
    pass "coding-agent session with a cwd elsewhere correctly ignored"
fi
if printf '%s\n' "$scan_out" | grep -q "$mention"; then
    fail "session that only QUOTES the workspace path wrongly flagged (scan: $scan_out)"
else
    pass "argv mentioning the workspace path is not a match (2026-08-03 class)"
fi
if printf '%s\n' "$scan_out" | grep -q "agent $eng2"; then
    fail "the engine was reported as an agent as well as via its pidfile"
else
    pass "engine not double-reported as an agent (lucidos-engine is off the list)"
fi
kill -KILL "$inside" "$outside" "$mention" "$eng2" 2>/dev/null; wait 2>/dev/null

# ── 6d. An agent that ignores SIGTERM is escalated to SIGKILL ────────────
# The polite ask is right first (an agent owns a git worktree and a child MCP
# server), but it must not be trusted: one that ignores it keeps ~150 MB and a
# node runtime for the life of the host.
echo "Test 6d: agent reap escalates SIGTERM to SIGKILL"
spawn_stubborn_sleeper; stubborn=$SLEEPER_PID
E2E_ORPHAN_AGENT_GRACE_S=1 _e2e_reap_orphans > /dev/null 2> "$OUT_DIR"/test-6d.err <<EOF
agent $stubborn
EOF
# SIGKILL is the last thing the reap does, so the corpse can still be unreaped
# when it returns. Production polls for the same reason (the reclaim path
# re-scans on a deadline); assert on death, not on the instant after the signal.
wait_until_dead "$stubborn"
if orphan_alive "$stubborn"; then
    fail "agent ignoring SIGTERM survived the reap (pid $stubborn)"
else
    pass "agent ignoring SIGTERM was escalated to SIGKILL"
fi
# Without this the test above passes on a fixture that simply died of SIGTERM,
# which is the one thing it is not meant to prove.
if grep -q "ignored SIGTERM, killed" "$OUT_DIR"/test-6d.err; then
    pass "the escalation path is what cleared it, not the polite SIGTERM"
else
    fail "SIGTERM alone cleared the fixture, so escalation was never exercised (stderr: $(cat "$OUT_DIR"/test-6d.err))"
fi
kill -KILL "$stubborn" 2>/dev/null; wait 2>/dev/null

# ── 6e. Teardown sweeps, so a clean run cleans up after itself ───────────
# The other half of the 2026-08-07 gap. Reclaim-time sweeping only ever fires
# when a LATER run finds a stale lock, so a run that ended cleanly (or whose
# successor never came) left its agents alive indefinitely.
echo "Test 6e: sweep_e2e_orphans reaps at teardown"
spawn_sleeper; leftover=$SLEEPER_PID
SYNTHETIC_PS="$leftover /Users/x/.local/bin/claude --settings /whatever/cc-settings.json"
SYNTHETIC_CWD="$leftover $E2E_WORKSPACE/.lucidos/worktrees/thread-xyz"
E2E_ORPHAN_AGENT_GRACE_S=1 sweep_e2e_orphans 2> "$OUT_DIR"/test-6e.err
sweep_rc=$?
SYNTHETIC_PS=""; SYNTHETIC_CWD=""
assert_eq "0" "$sweep_rc" "teardown sweep returns 0 (never reds a green run)"
if orphan_alive "$leftover"; then
    fail "teardown sweep left the agent alive (pid $leftover)"
else
    pass "teardown sweep reaped the leftover agent"
fi
if grep -q "orphan: agent $leftover" "$OUT_DIR"/test-6e.err; then
    pass "teardown sweep logs what it reaped (never a silent kill)"
else
    fail "teardown sweep killed silently (stderr: $(cat "$OUT_DIR"/test-6e.err))"
fi
kill -KILL "$leftover" 2>/dev/null; wait 2>/dev/null

# ── 6f2. A trailing slash on the workspace root must not disarm the scan ──
# `_e2e_workspace_dir` returns $E2E_WORKSPACE verbatim, so an operator export
# with a trailing slash would make the `<root>/*` prefix read `<root>//*` and
# match nothing. Same disarming class as the whitespace browsers path.
echo "Test 6f2: a trailing slash on the workspace root still matches"
if _e2e_path_under "/a/b/c/d" "/a/b/c/"; then
    pass "trailing slash on the root still matches a path beneath it"
else
    fail "a trailing slash silently disarmed the cwd prefix test"
fi
if _e2e_path_under "/a/b/cx/d" "/a/b/c"; then
    fail "sibling directory '/a/b/cx' wrongly matched root '/a/b/c'"
else
    pass "a sibling sharing a name prefix is not 'under' the root"
fi

# ── 6f. An empty process feed sweeps nothing ─────────────────────────────
# The seam fails closed (see `_e2e_orphan_ps`): an empty feed means "no
# candidates", never "fall back to real ps". A regression here puts the host's
# whole process table into a real kill, which is exactly what happened to
# webkit_reaper_test.sh on 2026-08-03.
echo "Test 6f: an empty process feed finds nothing to sweep"
SYNTHETIC_PS=""; SYNTHETIC_CWD=""
assert_eq "" "$(_e2e_list_orphans)" "empty feed yields no orphans"

# ── 7. Release only removes a lock we own ────────────────────────────────
echo "Test 7: release does not remove another owner's lock"
reset_lock_dir
cat > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" <<EOF
PID=$$
THREAD_ID=other
WORKTREE=/tmp/other
STARTED=2020-01-01T00:00:00Z
SCRIPT=e2e-api
EOF
unset E2E_LOCK_OWNED   # pretend we never acquired
release_e2e_lock
assert_eq "1" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "other-owner lock preserved"

# ── 8. Release removes our own lock ──────────────────────────────────────
echo "Test 8: release removes our own lock"
reset_lock_dir
acquire_e2e_lock e2e-browser >/dev/null 2>&1
release_e2e_lock
assert_eq "0" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "own lock removed"

# ── 9. Entry-point script bails out when the lock is held (live PID) ──────
# Simulates the original incident at the entry-point level: hold the lock from a
# "first" session (alive PID), then invoke e2e-browser.sh as the "second" and
# verify it exits before touching the workspace.
echo "Test 9: e2e-browser.sh bails out when lock held"
reset_lock_dir
cat > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" <<EOF
PID=$$
THREAD_ID=test-holder
WORKTREE=/tmp/holder
STARTED=$(date -u +%Y-%m-%dT%H:%M:%SZ)
SCRIPT=e2e-browser
EOF
"$PROJECT_DIR/scripts/e2e-browser.sh" -f none.spec.ts >"$OUT_DIR"/test-9.out 2>&1
script_rc=$?
assert_eq "1" "$script_rc" "e2e-browser.sh exits 1"
if grep -q "another e2e run is in progress" "$OUT_DIR"/test-9.out; then
    pass "e2e-browser.sh prints conflict message"
else
    fail "e2e-browser.sh missing conflict message"
    echo "  ---"; cat "$OUT_DIR"/test-9.out; echo "  ---"
fi
# It must NOT have started the workspace (no engine boot, no DB reset).
if ! grep -qE "Starting e2e workspace|Engine already running|Resetting database" "$OUT_DIR"/test-9.out; then
    pass "e2e-browser.sh aborted before workspace startup"
else
    fail "e2e-browser.sh started workspace despite lock"
fi
assert_eq "1" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "held lock preserved"
rm -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock"

# ── 10-18. The lock announces itself (the E2ELock* domain events) ────────
# A hold now emits E2ELockAcquired when it starts and E2ELockReleased when it
# ends, so a run that LOST the lock can subscribe with `lucidos await-event` and
# end its turn instead of busy-waiting (2026-08-09: two losers hand-rolled sleep
# loops, one of them a 40 minute foreground tool call).
#
# Everything below is about the emit being invisible when it fails. An e2e run
# must never go red, and an EXIT trap must never stall, because the engine was
# briefly unreachable.

echo ""
echo "Test 10: the emit guard keeps this suite off the developer's live workspace"
# $E2E_LOCK_DIR_OVERRIDE is what suppresses the emit, and this file exports it
# for the whole run. Without the guard, every test above would POST E2ELock*
# events into whichever workspace the developer is sitting in.
: > "$CAPTURE"
reset_lock_dir
acquire_e2e_lock e2e-browser >/dev/null 2>&1
release_e2e_lock
if [ -s "$CAPTURE" ]; then
    fail "a guarded acquire/release called lucidos: $(cat "$CAPTURE")"
else
    pass "no emit while E2E_LOCK_DIR_OVERRIDE is set"
fi

echo ""
echo "Test 11: acquire and release announce themselves"
reset_emit_sandbox
(
    unset E2E_LOCK_DIR_OVERRIDE
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    # acquire_e2e_lock backgrounds its announcement (it is holding the lock with
    # no teardown armed), so reap it before the parent reads $CAPTURE. `wait`
    # with no args is exact: this subshell has no other children.
    wait
    release_e2e_lock
)
acq="$(emit_payload E2ELockAcquired)"
rel="$(emit_payload E2ELockReleased)"
case "$acq" in
    *'"script":"e2e-browser"'*'"reclaimed":false'*) pass "acquire emits E2ELockAcquired for a free lock" ;;
    *) fail "wrong or missing E2ELockAcquired payload: $acq" ;;
esac
case "$rel" in
    *'"script":"e2e-browser"'*'"outcome":"released"'*) pass "release emits E2ELockReleased naming the script" ;;
    *) fail "wrong or missing E2ELockReleased payload: $rel" ;;
esac
case "$rel" in
    *'"thread_id":"'*'"worktree":"'*) pass "payload carries the thread id and worktree a waiter filters on" ;;
    *) fail "release payload lacks thread_id/worktree: $rel" ;;
esac
if printf '%s' "$rel" | grep -q '"held_secs":[0-9]'; then
    pass "release payload carries how long the lock was held"
else
    fail "release payload has no numeric held_secs: $rel"
fi
# The ordering invariant. A waiter woken by the release retries immediately, so
# waking one while the lock file is still there would spend one of its ten
# consecutive subscriptions on nothing.
if [ "$(emit_marker E2ELockReleased)" = "LOCKFILE_ABSENT" ]; then
    pass "the release is announced only after the lock file is gone"
else
    fail "E2ELockReleased fired while the lock file was still present"
fi
if [ "$(emit_marker E2ELockAcquired)" = "LOCKFILE_PRESENT" ]; then
    pass "the acquire is announced only after the lock file is written"
else
    fail "E2ELockAcquired fired before the lock file existed"
fi

echo ""
echo "Test 12: reclaiming a dead owner's lock announces THAT hold ending"
# A waiter is blocked on the hold, not on the process, and a hold whose owner
# died is over just as finally. Nothing else would ever announce it: the dead
# owner's EXIT trap never ran.
reset_emit_sandbox
cat > "$EMIT_LOCK" <<EOF
PID=999999
THREAD_ID=ghost
WORKTREE=/tmp/ghost
STARTED=2020-01-01T00:00:00Z
SCRIPT=e2e-api
EOF
(
    unset E2E_LOCK_DIR_OVERRIDE
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    wait          # both reclaim announcements are backgrounded
)
rel="$(emit_payload E2ELockReleased)"
case "$rel" in
    *'"outcome":"reclaimed"'*) pass "reclaim emits E2ELockReleased with outcome=reclaimed" ;;
    *) fail "reclaim did not announce the release: $rel" ;;
esac
case "$rel" in
    *'"script":"e2e-api"'*'"thread_id":"ghost"'*) pass "the payload describes the DEAD owner, not the reclaimer" ;;
    *) fail "reclaim payload describes the wrong hold: $rel" ;;
esac
# This fixture predates STARTED_EPOCH, exactly like every lock file written
# before it existed. The field must be omitted rather than emitted empty, which
# would be invalid JSON and would lose the whole event.
if printf '%s' "$rel" | grep -q 'held_secs'; then
    fail "a lock file with no STARTED_EPOCH still emitted held_secs: $rel"
else
    pass "no STARTED_EPOCH means held_secs is omitted, not emitted empty"
fi
case "$(emit_payload E2ELockAcquired)" in
    *'"reclaimed":true'*) pass "the reclaimer's own acquire is flagged reclaimed" ;;
    *) fail "reclaim acquire not flagged: $(emit_payload E2ELockAcquired)" ;;
esac
rm -f "$EMIT_LOCK"

echo ""
echo "Test 13: an emit that FAILS never reds the run or strands the lock"
reset_emit_sandbox
(
    unset E2E_LOCK_DIR_OVERRIDE
    STUB_EXIT=1
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1 || exit 90
    release_e2e_lock || exit 91
)
rc="$?"
assert_eq "0" "$rc" "acquire + release stay rc 0 when the emit exits non-zero"
assert_eq "0" "$([ -f "$EMIT_LOCK" ] && echo 1 || echo 0)" "the lock is still released when the emit fails"
drain_emit E2ELockAcquired

echo ""
echo "Test 14: an emit that HANGS is abandoned, not waited out"
# release_e2e_lock runs inside an EXIT trap. The CLI's own reqwest default is
# 30s, and a `lucidos` wedged before its HTTP client exists is not covered by it
# at all, so the bound has to live here.
reset_emit_sandbox
started="$(date +%s)"
(
    unset E2E_LOCK_DIR_OVERRIDE
    # Far past the 1s bound, but short enough that the two `sleep`s the killed
    # stubs leave re-parented do not outlive the suite by much.
    STUB_SLEEP_S=8
    E2E_LOCK_EMIT_TIMEOUT_S=1
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    release_e2e_lock
) 2> "$OUT_DIR"/test-14.err
rc="$?"
elapsed=$(( $(date +%s) - started ))
assert_eq "0" "$rc" "a hanging emit still returns 0"
if [ "$elapsed" -lt 10 ]; then
    pass "the hanging release emit cost ${elapsed}s, not 8 (bounded at 1s)"
else
    fail "teardown stalled ${elapsed}s on a hanging emit"
fi
assert_eq "0" "$([ -f "$EMIT_LOCK" ] && echo 1 || echo 0)" "the lock is still released past a hanging emit"
if grep -q "emit exceeded 1s and was abandoned" "$OUT_DIR"/test-14.err; then
    pass "the abandonment is logged, never silent"
else
    fail "a hanging emit was abandoned silently (stderr: $(cat "$OUT_DIR"/test-14.err))"
fi
drain_emit E2ELockAcquired

echo ""
echo "Test 14b: acquire does not block on its own announcement"
# acquire_e2e_lock returns having TAKEN the lock, and both entry points install
# their teardown only afterwards (scripts/e2e.sh, setup_e2e_session), so anything
# it blocks on widens the window in which an interrupt leaves a stale lock nobody
# releases and no waiter hears about. A wedged engine would have made that window
# 10s on the reclaim path, which announces twice. Hence the backgrounding, and
# hence this test: the release emit may block (its trap holds no lock), the
# acquire emit may not.
reset_emit_sandbox
started="$(date +%s)"
(
    unset E2E_LOCK_DIR_OVERRIDE
    STUB_SLEEP_S=8
    E2E_LOCK_EMIT_TIMEOUT_S=5
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    # Deliberately no `wait` here: the whole point is that the caller gets on
    # with arming its teardown instead of waiting for a decorative event.
) 2>/dev/null
elapsed=$(( $(date +%s) - started ))
if [ "$elapsed" -lt 3 ]; then
    pass "acquire returned in ${elapsed}s with the emit wedged (stale-lock window not widened)"
else
    fail "acquire blocked ${elapsed}s on its announcement, widening the stale-lock window"
fi
# It still has to HAPPEN, just not on the caller's clock. Draining it here also
# keeps its write out of the next case's capture (see drain_emit).
drain_emit E2ELockAcquired
if [ -n "$(emit_call E2ELockAcquired)" ]; then
    pass "the announcement still landed, just off the critical path"
else
    fail "backgrounding lost the announcement entirely"
fi
rm -f "$EMIT_LOCK"

echo ""
echo "Test 14c: a short run cannot persist Released before Acquired"
# The acquire announcement is backgrounded and the release one is not, so a run
# that ends quickly could otherwise land them out of order and show a lock that
# was released and then taken and never given back. A waiter does not care (it
# watches only for a release), but a timeline that lies is the whole reason the
# acquire event exists. Deterministic, not hopeful: the stub delays ONLY
# E2ELockAcquired, so without the ordering the release always wins.
reset_emit_sandbox
(
    unset E2E_LOCK_DIR_OVERRIDE
    STUB_PREDELAY_EVENT=E2ELockAcquired
    STUB_PREDELAY_S=1
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    release_e2e_lock          # no `wait`: release itself must do the ordering
)
# Emit calls only. This asserts the order two EVENTS were persisted in, and the
# capture also holds the stand-down, whose own third field is a flag rather than
# an event type.
order="$(awk -F'\t' '$1 == "events" && $2 == "emit" { print $3 }' "$CAPTURE" | tr '\n' ' ')"
case "$order" in
    "E2ELockAcquired E2ELockReleased "*)
        pass "acquired then released, in that order ($order)" ;;
    *)
        fail "lock events persisted out of order: $order" ;;
esac
rm -f "$EMIT_LOCK"

echo ""
echo "Test 15: no lucidos on PATH is a clean no-op"
reset_emit_sandbox
(
    unset E2E_LOCK_DIR_OVERRIDE
    PATH="/usr/bin:/bin"
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1 || exit 90
    release_e2e_lock || exit 91
) > "$OUT_DIR"/test-15.out 2>&1
rc="$?"
assert_eq "0" "$rc" "acquire + release stay rc 0 with no lucidos CLI"
assert_eq "0" "$([ -f "$EMIT_LOCK" ] && echo 1 || echo 0)" "the lock is still released with no lucidos CLI"
if [ -s "$OUT_DIR"/test-15.out ]; then
    fail "a missing CLI produced noise: $(cat "$OUT_DIR"/test-15.out)"
else
    pass "a missing CLI is silent (a manual run outside a workspace is normal)"
fi

echo ""
echo "Test 16: the payload survives a worktree path full of JSON metacharacters"
# WORKTREE is `$PWD` at acquire time, so it is arbitrary. release_events.sh gets
# away with embedding raw literals because its values are fixed step ids and
# N.N.N versions; that is not true here, and an unescaped quote would lose the
# whole event to a parse error at the engine.
reset_emit_sandbox
WEIRD_DIR="$TMPROOT/"'q"uote and\back'
mkdir -p "$WEIRD_DIR"
(
    unset E2E_LOCK_DIR_OVERRIDE
    cd "$WEIRD_DIR" || exit 90
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    wait
    release_e2e_lock
)
rel="$(emit_payload E2ELockReleased)"
if printf '%s' "$rel" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
    pass "a worktree with a quote and a backslash still yields parseable JSON"
else
    fail "payload is not valid JSON: $rel"
fi
if printf '%s' "$rel" | python3 -c \
    'import json,sys; d=json.load(sys.stdin); sys.exit(0 if d["worktree"].endswith("q\"uote and\\back") else 1)' \
    2>/dev/null; then
    pass "the escaped path round-trips to the real directory name"
else
    fail "the worktree did not round-trip: $rel"
fi

echo ""
echo "Test 17: the refusal teaches the subscribe path, not a sleep loop"
# An agent that never loaded the e2e-lock-wait skill has only this text to go on,
# and on 2026-08-09 two of them read the old wording and wrote retry loops.
reset_lock_dir
acquire_e2e_lock e2e-browser >/dev/null 2>&1
acquire_e2e_lock e2e-api > "$OUT_DIR"/test-17.out 2>&1
if grep -q "await-event --on E2ELockReleased" "$OUT_DIR"/test-17.out; then
    pass "the refusal spells out the await-event command"
else
    fail "the refusal does not name await-event"
    echo "  ---"; cat "$OUT_DIR"/test-17.out; echo "  ---"
fi
if grep -qi "do NOT sleep, poll, or write a retry loop" "$OUT_DIR"/test-17.out; then
    pass "the refusal names the anti-pattern it is replacing"
else
    fail "the refusal does not warn against polling"
fi
if grep -q "e2e-lock-wait" "$OUT_DIR"/test-17.out; then
    pass "the refusal points at the skill carrying the full rules"
else
    fail "the refusal does not point at the skill"
fi
release_e2e_lock

echo ""
echo "Test 18: the refusal warns when the holder's release cannot reach us"
# The lock is shared across every workspace on the machine, but an emit lands in
# the emitting subprocess's own $LUCIDOS_WORKSPACE. A waiter that subscribes to a
# holder in another workspace waits out its whole timeout, so say so up front.
reset_lock_dir
mkdir -p "$TMPROOT/ws-a" "$TMPROOT/ws-b"
cat > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" <<EOF
PID=$$
THREAD_ID=other
WORKTREE=$TMPROOT/ws-b/.lucidos/worktrees/thread-x
STARTED=2020-01-01T00:00:00Z
STARTED_EPOCH=1577836800
SCRIPT=e2e-browser
EOF
( LUCIDOS_WORKSPACE="$TMPROOT/ws-a"; acquire_e2e_lock e2e-api ) > "$OUT_DIR"/test-18a.out 2>&1
if grep -q "not reach this thread" "$OUT_DIR"/test-18a.out; then
    pass "a holder in another workspace is called out in the refusal"
else
    fail "no cross-workspace note for a holder elsewhere"
    echo "  ---"; cat "$OUT_DIR"/test-18a.out; echo "  ---"
fi
cat > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" <<EOF
PID=$$
THREAD_ID=other
WORKTREE=$TMPROOT/ws-a/.lucidos/worktrees/thread-x
STARTED=2020-01-01T00:00:00Z
STARTED_EPOCH=1577836800
SCRIPT=e2e-browser
EOF
# shellcheck disable=SC2030 # local to this case is the point: the library reads it in the same subshell, and the next case must not inherit it
( LUCIDOS_WORKSPACE="$TMPROOT/ws-a"; acquire_e2e_lock e2e-api ) > "$OUT_DIR"/test-18b.out 2>&1
if grep -q "not reach this thread" "$OUT_DIR"/test-18b.out; then
    fail "a same-workspace holder was wrongly reported as unreachable"
else
    pass "a holder in THIS workspace gets no note (its release does wake us)"
fi
# Cannot tell is not the same as another workspace: a manual terminal run has no
# $LUCIDOS_WORKSPACE, and guessing there would warn every such run for nothing.
( unset LUCIDOS_WORKSPACE; acquire_e2e_lock e2e-api ) > "$OUT_DIR"/test-18c.out 2>&1
if grep -q "not reach this thread" "$OUT_DIR"/test-18c.out; then
    fail "warned about the workspace gap with no LUCIDOS_WORKSPACE to compare against"
else
    pass "no workspace to compare against means no claim either way"
fi
rm -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock"

echo ""
echo "Test 18b: a removal that FAILS is not announced as a release"
# `rm -f` is silent about a missing file but not about a permission or
# filesystem error. Announcing through one wakes every waiter onto a lock that is
# still held: each retries, is refused, and spends one of its ten consecutive
# subscriptions on a release that never happened. `rm` is shadowed for the
# subshell, the same seam style as test 5's stubbed reaper.
reset_emit_sandbox
(
    unset E2E_LOCK_DIR_OVERRIDE
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    wait                                 # let the backgrounded acquire emit land
    : > "$CAPTURE"                       # then drop it
    rm() { return 1; }
    release_e2e_lock
) 2> "$OUT_DIR"/test-18d.err
assert_eq "0" "$?" "a failed removal still returns 0 (never reds a run)"
assert_eq "1" "$([ -f "$EMIT_LOCK" ] && echo 1 || echo 0)" "the lock file is still there, as the failure implies"
if [ -n "$(emit_call E2ELockReleased)" ]; then
    fail "announced a release the removal never performed: $(emit_call E2ELockReleased)"
else
    pass "no E2ELockReleased when the lock was not actually released"
fi
if grep -q "STILL HELD" "$OUT_DIR"/test-18d.err; then
    pass "the failure is reported loudly instead of passing for a release"
else
    fail "a failed removal was silent (stderr: $(cat "$OUT_DIR"/test-18d.err))"
fi
rm -f "$EMIT_LOCK"

echo ""
echo "Test 19: release skips a lock another run has taken over, but not a broken one"
# "Only removes the lock if we own it" used to mean only that this shell had set
# E2E_LOCK_OWNED. A run whose pid another process could not reach with `kill -0`
# (a different user on the host) has its lock legitimately reclaimed, and this
# function would then delete the new owner's file on the way out.
reset_lock_dir
cat > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" <<EOF
PID=999999
THREAD_ID=someone-else
WORKTREE=/tmp/someone-else
STARTED=2020-01-01T00:00:00Z
STARTED_EPOCH=1577836800
SCRIPT=e2e-api
EOF
E2E_LOCK_OWNED="$E2E_LOCK_DIR_OVERRIDE/e2e.lock"
release_e2e_lock 2> "$OUT_DIR"/test-19.err
assert_eq "1" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "a lock now held by another PID is left alone"
if grep -q "held by PID 999999 now, not us" "$OUT_DIR"/test-19.err; then
    pass "the skip is announced, never silent"
else
    fail "release skipped silently (stderr: $(cat "$OUT_DIR"/test-19.err))"
fi
# The other direction matters more: a lock file we cannot read a pid out of must
# still be removed. Leaving it would wedge every future run behind a refusal
# whose stale-owner branch needs a pid it will never find.
reset_lock_dir
printf 'SCRIPT=e2e-api\n' > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock"
E2E_LOCK_OWNED="$E2E_LOCK_DIR_OVERRIDE/e2e.lock"
release_e2e_lock 2>/dev/null
assert_eq "0" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "a lock with no readable PID is still removed (no wedge)"

echo ""
echo "Test 19b: a lock file with no trailing newline keeps its last field"
# `while read` returns non-zero on a final line with no newline, having already
# filled the variables, so the plain form drops it. SCRIPT is written last, so
# the lost field is the one the refusal names.
printf 'PID=123\nWORKTREE=/tmp/w\nSCRIPT=e2e-api' > "$TMPROOT/lock-no-newline"
_e2e_read_lock_file "$TMPROOT/lock-no-newline"
assert_eq "123" "$_E2E_LK_PID" "an unterminated lock file still yields earlier fields"
assert_eq "e2e-api" "$_E2E_LK_SCRIPT" "and still yields the LAST field"
rm -f "$TMPROOT/lock-no-newline"

echo ""
echo "Test 20: a malformed STARTED_EPOCH degrades quietly"
# `_e2e_lock_held_secs` is read through a command substitution inside an EXIT
# trap under `set -e`, so it must exit 0 whatever it is handed, and it must not
# leak bash's "integer expression expected" over a lock file it has already
# given up on.
for bad in "" "not-a-number" "99999999999999999999999" "-5"; do
    out="$(_e2e_lock_held_secs "$bad" 2>"$OUT_DIR"/test-20.err)"
    rc=$?
    assert_eq "0" "$rc" "held_secs exits 0 for STARTED_EPOCH='$bad'"
    assert_eq "" "$out" "held_secs is empty for STARTED_EPOCH='$bad'"
    if [ -s "$OUT_DIR"/test-20.err ]; then
        fail "held_secs leaked to stderr for '$bad': $(cat "$OUT_DIR"/test-20.err)"
    else
        pass "held_secs is silent for STARTED_EPOCH='$bad'"
    fi
done
out="$(_e2e_lock_held_secs "$(( $(date +%s) - 42 ))")"
assert_eq "42" "$out" "held_secs still measures a well-formed epoch"

echo ""
echo "Test 21: a failed lock-file read does not abort the EXIT trap under \`set -e\`"
# release_e2e_lock runs from an EXIT trap in every entry script, all of which set
# -e, and it reads the lock file with a bare call. A bare call to a function that
# returns non-zero takes `set -e` with it, so a read failing there truncates the
# rest of teardown rather than reporting anything.
#
# The window is real but not reproducible by timing (the file has to vanish
# between the `-f` test and the read), so the read is stubbed to fail, exactly as
# test 5 stubs the reaper. The stub clears the fields first, mirroring the real
# one, so the case does not lean on whatever the previous test left behind.
#
# The subshell is a BARE statement, deliberately. Wrapping it in `if ( ... );
# then` reads naturally and is inert: bash suppresses errexit for everything in
# an `if` condition, subshells and the functions they call included, so the
# earlier shape passed identically with and without the fix. The same rule is why
# `acquire_e2e_lock` needs no test here: both entry points invoke it as
# `acquire_e2e_lock <label> || exit 1`, and a `||` list suppresses errexit inside
# the function too.
rm -f "$OUT_DIR"/test-21.marker
reset_lock_dir
acquire_e2e_lock e2e-browser >/dev/null 2>&1
( set -e
  _e2e_read_lock_file() {
      _E2E_LK_PID=""; _E2E_LK_THREAD=""; _E2E_LK_WORKTREE=""
      _E2E_LK_STARTED=""; _E2E_LK_STARTED_EPOCH=""; _E2E_LK_SCRIPT=""
      return 1
  }
  release_e2e_lock >/dev/null 2>&1
  echo reached > "$OUT_DIR"/test-21.marker )
assert_eq "0" "$?" "release_e2e_lock returns to its caller when the read fails"
assert_eq "reached" "$(cat "$OUT_DIR"/test-21.marker 2>/dev/null)" "the teardown step after release still ran"
release_e2e_lock >/dev/null 2>&1

echo ""
echo "Test 22: a hold's release is addressed to the workspace that TOOK the lock"
# `lucidos events emit` writes to the emitting subprocess's own
# $LUCIDOS_WORKSPACE, and the entry scripts repoint that variable BETWEEN a
# hold's two announcements: acquire runs first, then `reset_e2e_database` reaches
# `setup_postgres`, which exports the e2e workspace into the entry script's own
# shell, and only then does the EXIT trap release. So the release used to be
# addressed to an engine that teardown had just stopped, in a workspace no waiter
# watches. The evidence it left, and what it cost, are in ADR 0057 and in the
# library's own announcing section; both are one place, so neither goes stale
# here.
#
# The re-export below IS the `setup_postgres` line, reproduced: the whole point
# is that it happens after the lock is taken and before it is given back.
reset_emit_sandbox
mkdir -p "$TMPROOT/ws-caller" "$TMPROOT/ws-e2e"
(
    unset E2E_LOCK_DIR_OVERRIDE
    # shellcheck disable=SC2030,SC2031 # both modifications are meant to be local: the subshell IS the entry script being simulated
    export LUCIDOS_WORKSPACE="$TMPROOT/ws-caller"
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    wait                                          # let the backgrounded acquire land
    export LUCIDOS_WORKSPACE="$TMPROOT/ws-e2e"    # setup_postgres, mid-run
    release_e2e_lock
)
assert_eq "WS=$TMPROOT/ws-caller" "$(emit_workspace E2ELockAcquired)" \
    "the acquire is addressed to the caller's workspace"
assert_eq "WS=$TMPROOT/ws-caller" "$(emit_workspace E2ELockReleased)" \
    "the release is addressed there too, not to the e2e workspace"
# The stand-down rides the same capture, and needs it for a second reason: the
# subscription it ends belongs to a thread in the CALLER's workspace, so a call
# addressed to the e2e one would reach an engine that has never heard of it.
assert_eq "WS=$TMPROOT/ws-caller" "$(standdown_workspace)" \
    "and so is the stand-down, which is where the subscription lives"
rm -f "$EMIT_LOCK"

echo ""
echo "Test 23: with no workspace at acquire, the release emits with none either"
# A terminal run has no $LUCIDOS_WORKSPACE, so the CLI resolves by walking up
# from the working directory. Pinning the capture as an empty STRING would be
# worse than not pinning at all: the CLI reads `env::var(..).ok()`, so `Some("")`
# is a workspace root and `.lucidos/ports` resolves against nothing. Absent has
# to stay absent, and the value the entry script exported later must not stand in
# for it.
reset_emit_sandbox
(
    unset E2E_LOCK_DIR_OVERRIDE
    unset LUCIDOS_WORKSPACE
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    wait
    # shellcheck disable=SC2031 # same as the case above: the subshell is the simulated entry script, and nothing outside reads this
    export LUCIDOS_WORKSPACE="$TMPROOT/ws-e2e"
    release_e2e_lock
)
assert_eq "WS_UNSET" "$(emit_workspace E2ELockAcquired)" \
    "the acquire carries no workspace, as the caller had none"
assert_eq "WS_UNSET" "$(emit_workspace E2ELockReleased)" \
    "and the release carries none either, rather than the e2e workspace"
rm -f "$EMIT_LOCK"

echo ""
echo "Test 24: taking the lock stands down this thread's watch for its release"
# The other half of the announce loop. A thread that lost the race subscribed to
# E2ELockReleased and ended its turn; when it later WINS the lock that watch is
# answered, because you cannot hold the lock and still need to be told it is
# free. On 2026-08-09 nothing said so: a thread subscribed at 18:31, took the
# lock itself at 18:38 and eight more times after that, had its change applied
# at 19:14, and was woken at 21:21 by the subscription it never stood down.
reset_emit_sandbox
(
    unset E2E_LOCK_DIR_OVERRIDE
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    wait
)
call="$(standdown_call)"
case "$call" in
    *"--on${TAB}E2ELockReleased"*)
        pass "the acquire stands down the watch for E2ELockReleased" ;;
    *)
        fail "the acquire did not stand its watch down: ${call:-<no call>}" ;;
esac
# NEVER `--all`. A thread may legitimately be waiting on something unrelated
# while it runs e2e (a release build, a sibling thread), and ending a watch
# nobody asked about is the harm ADR 0052 exists to prevent. The lock may only
# end the watch it just answered.
case "$call" in
    *"--all"*) fail "the stand-down reached for --all: $call" ;;
    *)         pass "and touches nothing else this thread is waiting on" ;;
esac
# Which workspace it is addressed to is Test 22's, where the mid-run repoint
# that makes the question interesting is already simulated.
rm -f "$EMIT_LOCK"

echo ""
echo "Test 25: a reclaim stands down BEFORE it announces the dead owner's release"
# Load-bearing ordering, and the one place it changes an outcome. The reclaim
# path emits an E2ELockReleased on the dead owner's behalf, which is exactly the
# event our own watch matches: still live when it lands, it wakes us onto a lock
# we are holding. Standing down first is what makes a reclaim unable to wake its
# own reclaimer.
reset_emit_sandbox
(
    unset E2E_LOCK_DIR_OVERRIDE
    E2E_LOCK_OWNED=""
    cat > "$EMIT_LOCK" <<EOF
PID=999999
THREAD_ID=ghost
WORKTREE=$TMPROOT/ws-dead
STARTED=2020-01-01T00:00:00Z
STARTED_EPOCH=1577836800
SCRIPT=e2e-api
EOF
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    wait
)
sequence="$(awk -F'\t' '{ print $1 "-" $2 }' "$CAPTURE" | tr '\n' ' ')"
case "$sequence" in
    "event-waits-cancel events-emit events-emit "*)
        pass "stood down first, then announced both ends of the handover ($sequence)" ;;
    *)
        fail "a reclaim announced a release before standing its own watch down: $sequence" ;;
esac
assert_eq "reclaimed" "$(emit_payload E2ELockReleased | sed -n 's/.*"outcome":"\([^"]*\)".*/\1/p')" \
    "and the release it announced is still the dead owner's"
rm -f "$EMIT_LOCK"

echo ""
echo "Test 26: no thread means no stand-down call at all"
# A human running e2e from a terminal has no $LUCIDOS_THREAD_ID and therefore no
# subscription. The CLI would refuse for that reason; not spawning the process
# is the same answer without the fork, and it keeps a manual run silent.
reset_emit_sandbox
(
    unset E2E_LOCK_DIR_OVERRIDE
    unset LUCIDOS_THREAD_ID
    E2E_LOCK_OWNED=""
    acquire_e2e_lock e2e-browser >/dev/null 2>&1
    wait
)
drain_emit E2ELockAcquired
if [ -n "$(standdown_call)" ]; then
    fail "a thread-less run still called the stand-down: $(standdown_call)"
else
    pass "no thread, no stand-down"
fi
assert_eq "1" "$([ -n "$(emit_call E2ELockAcquired)" ] && echo 1 || echo 0)" \
    "and the acquire is still announced, which needs no thread"
rm -f "$EMIT_LOCK"

# ── Summary ──────────────────────────────────────────────────────────────
echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
