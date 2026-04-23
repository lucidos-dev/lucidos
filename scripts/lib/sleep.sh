#!/bin/bash
# Clamshell (lid-close) sleep prevention for macOS Apple Silicon.
# caffeinate -s only works on AC power and NEVER prevents clamshell sleep.
# Only `pmset disablesleep 1` prevents lid-close sleep.
# Lock directory coordinates multiple workspaces: only the last to exit
# re-enables sleep.
# macOS-only: uses `md5 -q` and `pmset` (no Linux equivalent needed).

SLEEP_LOCK_DIR="/tmp/cognos-sleep-locks"
SUDOERS_FILE="/etc/sudoers.d/cognos-pmset"

# Ensure passwordless sudo for pmset disablesleep.
# Creates a sudoers.d entry on first run (requires one sudo prompt).
# After that, sudo -n always works — no prompts, even from non-TTY contexts.
ensure_sudoers_pmset() {
    if [ -f "$SUDOERS_FILE" ]; then
        return 0
    fi

    local rule="%admin ALL=(root) NOPASSWD: /usr/bin/pmset disablesleep 0, /usr/bin/pmset disablesleep 1"

    if echo "$rule" | sudo -n tee "$SUDOERS_FILE" >/dev/null 2>&1; then
        sudo -n chmod 0440 "$SUDOERS_FILE" 2>/dev/null || { sudo -n rm -f "$SUDOERS_FILE" 2>/dev/null; return 1; }
        return 0
    fi

    if [ -t 0 ]; then
        echo ""
        echo "One-time setup: allow CognOS to prevent lid-close sleep without password."
        echo "This creates $SUDOERS_FILE (only allows pmset disablesleep)."
        echo "$rule" | sudo tee "$SUDOERS_FILE" >/dev/null && sudo chmod 0440 "$SUDOERS_FILE"
        return $?
    fi

    return 1
}

# Remove lock files whose owning process has died.
cleanup_stale_sleep_locks() {
    for lock_file in "$SLEEP_LOCK_DIR"/*; do
        [ -f "$lock_file" ] || continue
        local lock_pid
        lock_pid="$(cat "$lock_file" 2>/dev/null || echo "")"
        if [ -n "$lock_pid" ] && ! kill -0 "$lock_pid" 2>/dev/null; then
            rm -f "$lock_file"
        fi
    done
}

# Release the lock for a workspace and re-enable sleep if no locks remain.
# $1 = workspace path (defaults to $WORKSPACE)
release_sleep_lock() {
    local ws="${1:-$WORKSPACE}"
    local ws_hash
    ws_hash="$(echo -n "$ws" | md5 -q)"
    rm -f "$SLEEP_LOCK_DIR/$ws_hash"

    cleanup_stale_sleep_locks

    if [ -z "$(ls -A "$SLEEP_LOCK_DIR" 2>/dev/null)" ]; then
        sudo -n pmset disablesleep 0 2>/dev/null || true
    fi
}
