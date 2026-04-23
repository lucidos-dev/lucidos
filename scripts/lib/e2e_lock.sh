#!/bin/bash
# Single-writer lock for the shared e2e-test workspace.
#
# Two CC sessions running Playwright concurrently against ~/workspaces/e2e-test
# crashed a 32 GB Mac on 2026-04-19 — WebKit GPU processes ballooned to 28 GB,
# the kernel page-thrashed, and the OS rebooted. This lock makes the second
# entrant exit cleanly instead of joining the pile-on.
#
# Lock file: <e2e-workspace>/.cognos/e2e.lock (or $E2E_LOCK_DIR_OVERRIDE for tests)
# Format:
#   PID=12345
#   THREAD_ID=<COGNOS_THREAD_ID or "unknown">
#   WORKTREE=<pwd at acquire time>
#   STARTED=<ISO 8601 UTC>
#   SCRIPT=<entry-point name>
#
# Stale locks (PID no longer alive) are reclaimed automatically.

E2E_LOCK_OWNED=""

# Resolve the lock file path. $E2E_LOCK_DIR_OVERRIDE is for tests; otherwise
# falls back to ~/workspaces/e2e-test/.cognos/.
_e2e_lock_path() {
    local dir="${E2E_LOCK_DIR_OVERRIDE:-${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}/.cognos}"
    mkdir -p "$dir" 2>/dev/null
    echo "$dir/e2e.lock"
}

# Atomic create-or-fail using noclobber. Returns 0 on success, non-zero if file exists.
_e2e_lock_write() {
    local lock_file="$1" script_name="$2"
    local thread_id="${COGNOS_THREAD_ID:-unknown}"
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

    # File exists — read all metadata in one pass (IFS== keeps everything after
    # the first '=' in $val, so worktree paths containing '=' survive).
    local existing_pid="" existing_thread="" existing_wt="" existing_started="" existing_script=""
    local key val
    while IFS== read -r key val; do
        case "$key" in
            PID)       existing_pid="$val" ;;
            THREAD_ID) existing_thread="$val" ;;
            WORKTREE)  existing_wt="$val" ;;
            STARTED)   existing_started="$val" ;;
            SCRIPT)    existing_script="$val" ;;
        esac
    done < "$lock_file"

    # Stale (dead PID) — reclaim and retry once.
    if [ -n "$existing_pid" ] && ! kill -0 "$existing_pid" 2>/dev/null; then
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
