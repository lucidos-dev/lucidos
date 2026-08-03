#!/bin/bash
# Single-writer lock for the shared e2e-test workspace.
#
# Two CC sessions running Playwright concurrently against ~/workspaces/e2e-test
# crashed a 32 GB Mac on 2026-04-19 — WebKit GPU processes ballooned to 28 GB,
# the kernel page-thrashed, and the OS rebooted. This lock makes the second
# entrant exit cleanly instead of joining the pile-on.
#
# Lock file: <e2e-workspace>/.lucidos/e2e.lock (or $E2E_LOCK_DIR_OVERRIDE for tests)
# Format:
#   PID=12345
#   THREAD_ID=<LUCIDOS_THREAD_ID or "unknown">
#   WORKTREE=<pwd at acquire time>
#   STARTED=<ISO 8601 UTC>
#   SCRIPT=<entry-point name>
#
# Reclaiming a stale lock is ORPHAN-SAFE, not blind. A "stale" lock is one whose
# owner PID is dead — but an INTERRUPTED run (killed before its EXIT trap could
# tear down) leaves orphaned e2e processes alive: Playwright/WebKit browser
# children and the e2e-test workspace engine, still holding their RSS. The
# nightly orchestrator re-spawned the full e2e suite THREE times on 2026-06-21,
# and each re-spawn reclaimed the "free" stale lock and stacked a fresh set of
# browsers on top of the orphans → 23.5 GB compressed + 14 GB swap, the machine
# pinned in critical memory pressure for 4+ hours.
#
# So before reclaiming a stale lock we SWEEP the prior run's orphans
# (deliberately, logged), then re-scan; we reclaim only once they are gone. If
# the sweep can't clear them we REFUSE rather than stack. The four states:
#   1. no lock file           → acquire
#   2. live-PID lock          → hard-fail (another run is live)
#   3. stale lock, no orphans → reclaim (as before)
#   4. stale lock + orphans   → sweep; reclaim if clean, else refuse

E2E_LOCK_OWNED=""

# Directory this lib lives in (scripts/lib), used to point operators at stop.sh
# in the refusal message. Resolved at source time; safe under `set -u`.
_E2E_LOCK_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd)"

# Resolve the lock file path. $E2E_LOCK_DIR_OVERRIDE is for tests; otherwise
# falls back to ~/workspaces/e2e-test/.lucidos/.
_e2e_lock_path() {
    local dir="${E2E_LOCK_DIR_OVERRIDE:-${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}/.lucidos}"
    mkdir -p "$dir" 2>/dev/null
    echo "$dir/e2e.lock"
}

# The e2e-test workspace directory ($E2E_WORKSPACE for tests; default otherwise).
_e2e_workspace_dir() {
    printf '%s' "${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}"
}

# ── orphan detection + sweep (the test seam) ────────────────────────────
# A stale lock means the prior run's controlling PID is dead, but an interrupted
# run can leave orphans alive (see the header). These helpers find and reap them.

# Process listing seam. Emits one "PID COMMAND" line per process. The test
# overrides this to feed synthetic rows for real sleeper PIDs without spawning
# browsers. `-Aww` = every process, unlimited width (long browser paths intact).
_e2e_orphan_ps() {
    ps -Aww -o pid=,command= 2>/dev/null
}

# Executable-path substrings that mark a process as a Playwright browser child.
# Matching by the browsers-cache path (NOT a bare "WebContent") is what keeps us
# off the user's own Safari/Chrome and unrelated WebKit consumers: the same
# discriminator webkit_reaper.sh uses, broadened from webkit to every browser
# engine because a dead run can orphan any of them. Honors
# PLAYWRIGHT_BROWSERS_PATH like the reaper.
#
# Tested against argv[0] ONLY, never the whole command line. See _e2e_list_orphans.
_e2e_orphan_browser_tokens() {
    local base
    if [ -n "${PLAYWRIGHT_BROWSERS_PATH:-}" ]; then
        base="${PLAYWRIGHT_BROWSERS_PATH%/}"
    else
        base="ms-playwright"
    fi
    printf '%s\n' "$base/webkit" "$base/chromium" "$base/firefox"
}

# Emit "KIND PID" for every LIVE orphan of a prior e2e run. KIND ∈ browser|engine.
# Browser children are matched by the cache-path substring; the engine is keyed on
# the e2e-test workspace's OWN engine.pid (so we never touch another workspace's
# engine). PID≤1 and our own shell are always skipped. Test seam: overridden by
# the test to inject fakes.
_e2e_list_orphans() {
    local self=$$
    local tokens
    tokens="$(_e2e_orphan_browser_tokens)"
    local pid command tok

    # Tokens are matched against argv[0], which is read up to the first space, so
    # a browsers path containing whitespace can never match and orphan detection
    # is effectively off. Say so out loud: a silently blind scan lets a new run
    # stack on live orphans, which is the pile-up this lock exists to prevent.
    # Only an operator override of PLAYWRIGHT_BROWSERS_PATH can reach this; the
    # default cache paths carry no spaces.
    while IFS= read -r tok; do
        case "$tok" in
            *[[:space:]]*)
                echo "[e2e-lock] WARNING: browser token '$tok' contains whitespace, so it can never match argv[0]. Orphan browser detection is effectively OFF. Use a PLAYWRIGHT_BROWSERS_PATH without spaces." >&2
                ;;
        esac
    done <<EOF
$tokens
EOF
    _e2e_orphan_ps | while read -r pid command; do
        case "$pid" in ''|*[!0-9]*) continue ;; esac
        [ "$pid" -le 1 ] && continue
        [ "$pid" = "$self" ] && continue
        # Iterate tokens via read (not `for tok in $(...)`) so a browsers-cache
        # path containing whitespace can't word-split the match.
        #
        # Each token is matched against argv[0] ONLY, never the whole command
        # line: a process is a browser child if the BINARY IT RUNS lives under
        # the browsers cache, not if its arguments mention that path. This
        # branch SIGKILLs unconditionally with no RSS threshold, so a full
        # command-line match is especially dangerous here. A Claude Code process
        # carries the engine's THREAD HISTORY inside a ~22 KB
        # --append-system-prompt argument, and on 2026-08-03 the sibling matcher
        # in webkit_reaper.sh killed two sessions that merely discussed these
        # paths.
        #
        # The one Playwright-owned process this deliberately stops matching is
        # the `pw_run.sh` wrapper shell, whose argv[0] is bash and whose path is
        # only argv[1]. That is fine and must not be "fixed" by widening back to
        # the whole command line: the wrapper holds a few MB, not the browser's
        # RSS, and since it runs the browser WITHOUT exec it simply exits once we
        # kill its child. Widening to argv[1] would put every `bash -c` whose
        # argument mentions these paths back in scope.
        while IFS= read -r tok; do
            [ -z "$tok" ] && continue
            case "${command%% *}" in
                *"$tok"*) echo "browser $pid"; break ;;
            esac
        done <<EOF
$tokens
EOF
    done

    # e2e-test workspace engine — keyed on its dedicated pidfile, liveness-gated.
    local engine_pid
    engine_pid="$(cat "$(_e2e_workspace_dir)/.lucidos/engine.pid" 2>/dev/null)"
    case "$engine_pid" in
        ''|*[!0-9]*) : ;;
        *)
            if [ "$engine_pid" -gt 1 ] && [ "$engine_pid" != "$self" ] \
               && kill -0 "$engine_pid" 2>/dev/null; then
                echo "engine $engine_pid"
            fi
            ;;
    esac
}

# Deliberate, logged sweep of the "KIND PID" lines from _e2e_list_orphans (read
# on stdin). Browser children: SIGKILL — the owning run is dead, free their RSS
# now (exactly what the webkit reaper does). Engine: SIGUSR1 — the engine ignores
# SIGTERM but exits 0 on SIGUSR1, and its supervisor then stops on that clean
# exit (the same mechanism stop.sh relies on; SIGKILL would just get respawned).
# Every reap prints a line — never a silent kill. Test seam: overridden to a
# no-op to exercise the refuse-on-survive path.
_e2e_reap_orphans() {
    local kind pid
    while read -r kind pid; do
        [ -z "$pid" ] && continue
        case "$kind" in
            browser)
                kill -KILL "$pid" 2>/dev/null \
                    && echo "[e2e-lock] reaped orphan browser pid=$pid (SIGKILL)" >&2
                ;;
            engine)
                kill -USR1 "$pid" 2>/dev/null \
                    && echo "[e2e-lock] signaled orphan engine pid=$pid to stop (SIGUSR1)" >&2
                ;;
        esac
    done
}

# Atomic create-or-fail using noclobber. Returns 0 on success, non-zero if file exists.
_e2e_lock_write() {
    local lock_file="$1" script_name="$2"
    local thread_id="${LUCIDOS_THREAD_ID:-unknown}"
    local started
    started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    (set -C; cat > "$lock_file" <<EOF
PID=$$
THREAD_ID=$thread_id
WORKTREE=$PWD
STARTED=$started
SCRIPT=$script_name
EOF
    ) 2>/dev/null
}

# acquire_e2e_lock <script-name>
# Returns 0 on success, 1 on conflict.
acquire_e2e_lock() {
    local script_name="${1:-e2e}"
    local lock_file
    lock_file="$(_e2e_lock_path)"

    if _e2e_lock_write "$lock_file" "$script_name"; then
        E2E_LOCK_OWNED="$lock_file"
        return 0
    fi

    # File exists — read all metadata in one pass (IFS='=' keeps everything after
    # the first '=' in $val, so worktree paths containing '=' survive).
    local existing_pid="" existing_thread="" existing_wt="" existing_started="" existing_script=""
    local key val
    while IFS='=' read -r key val; do
        case "$key" in
            PID)       existing_pid="$val" ;;
            THREAD_ID) existing_thread="$val" ;;
            WORKTREE)  existing_wt="$val" ;;
            STARTED)   existing_started="$val" ;;
            SCRIPT)    existing_script="$val" ;;
        esac
    done < "$lock_file"

    # Stale (dead PID) — but a dead run can leave ORPHANED e2e processes alive.
    # Sweep them before reclaiming; refuse if the sweep can't clear them. (State 4.)
    if [ -n "$existing_pid" ] && ! kill -0 "$existing_pid" 2>/dev/null; then
        local orphans
        orphans="$(_e2e_list_orphans)"
        if [ -n "$orphans" ]; then
            echo "[e2e-lock] stale lock (owner PID $existing_pid is dead) but the prior" >&2
            echo "[e2e-lock] run left orphaned e2e processes still alive — sweeping before" >&2
            echo "[e2e-lock] reclaim (blindly reclaiming would stack a fresh run on top):" >&2
            printf '%s\n' "$orphans" | sed 's/^/[e2e-lock]   orphan: /' >&2
            _e2e_reap_orphans <<EOF
$orphans
EOF
            # Poll until the sweep clears them. Browser SIGKILL is near-instant;
            # the engine's SIGUSR1 graceful shutdown can take up to its ~10s budget.
            local timeout_s="${E2E_ORPHAN_REAP_TIMEOUT_S:-15}"
            case "$timeout_s" in ''|*[!0-9]*) timeout_s=15 ;; esac
            local deadline=$(( $(date +%s) + timeout_s ))
            while [ "$(date +%s)" -lt "$deadline" ]; do
                orphans="$(_e2e_list_orphans)"
                [ -z "$orphans" ] && break
                sleep 0.5
            done
            if [ -n "$orphans" ]; then
                local ws
                ws="$(_e2e_workspace_dir)"
                echo "" >&2
                echo "ERROR: a prior e2e run died and left orphaned processes that the" >&2
                echo "automatic sweep could not stop within ${timeout_s}s:" >&2
                printf '%s\n' "$orphans" | sed 's/^/  /' >&2
                echo "" >&2
                echo "Reclaiming the lock now would stack a fresh run on top of them and" >&2
                echo "exhaust host memory (3 nightly pile-ups hit 23.5 GB + 14 GB swap on" >&2
                echo "2026-06-21). Refusing to start." >&2
                echo "" >&2
                echo "Clean up the listed PIDs, then re-run. The e2e-test workspace can be" >&2
                echo "stopped with:" >&2
                echo "  ${_E2E_LOCK_LIB_DIR%/lib}/stop.sh -w \"$ws\"" >&2
                echo "and any leftover Playwright browser PIDs with: kill -KILL <pid>" >&2
                echo "" >&2
                echo "Lock file: $lock_file" >&2
                return 1
            fi
            echo "[e2e-lock] orphans reaped — reclaiming the stale lock" >&2
        fi
        rm -f "$lock_file"
        if _e2e_lock_write "$lock_file" "$script_name"; then
            E2E_LOCK_OWNED="$lock_file"
            return 0
        fi
    fi

    echo ""
    echo "ERROR: another e2e run is in progress on the e2e-test workspace."
    echo "  Owner:    PID ${existing_pid:-unknown} (script: ${existing_script:-unknown})"
    echo "  Thread:   ${existing_thread:-unknown}"
    echo "  Worktree: ${existing_wt:-unknown}"
    echo "  Started:  ${existing_started:-unknown}"
    echo ""
    echo "Concurrent e2e runs against the shared e2e-test workspace race on"
    echo "Playwright browser processes — two sessions on a 32 GB Mac OOM'd the"
    echo "system on 2026-04-19. Wait for the other run to finish, or stop it."
    echo ""
    echo "Lock file: $lock_file"
    return 1
}

# release_e2e_lock — only removes the lock if we own it. Safe in EXIT traps.
release_e2e_lock() {
    local owned="${E2E_LOCK_OWNED:-}"
    if [ -n "$owned" ] && [ -f "$owned" ]; then
        rm -f "$owned"
    fi
    E2E_LOCK_OWNED=""
}
