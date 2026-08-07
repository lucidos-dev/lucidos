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
$eng2 /Users/x/target/release/launch/e2e-test-hooks/lucidos-engine"
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

# ── Summary ──────────────────────────────────────────────────────────────
echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
