# shellcheck shell=bash
# WebKit RSS reaper — host-resource safety net for browser e2e runs.
#
# The Playwright WebKit network process can stall the first navigation of a fresh
# context on macOS system-proxy (PAC/WPAD) auto-discovery during `mobile-webkit`
# runs (root cause is browser-side, not the dev server/engine; fixed at the source
# by the explicit `proxy` on the mobile-webkit project in playwright.config.ts —
# see docs/e2e-test-decisions.md "mobile-webkit navigation wedge"). The suite also
# self-heals via gotoWithRetry + retries:1. But a wedged WebContent process SITS
# ON ITS RSS without exiting; under nightly load several pile up and exhaust host
# memory — froze a 48 GB Mac on 2026-06-07 (and a WebKit GPU child OOM-rebooted a
# 32 GB Mac on 2026-04-19, see e2e_lock.sh). This reaper stays as the host-memory
# safety net regardless of how often the wedge actually fires.
#
# NOTE (2026-06-24): the PRIMARY orphan-prevention is now graceful engine
# teardown — on cancel/shutdown the engine SIGTERMs the CC process group and
# waits before SIGKILL so the Playwright runner closes its own (detached)
# browsers (see crates/lucidos-engine/src/runtime/spawn_env.rs ::
# graceful_kill_child_process_group, and ADR 0014's 2026-06-24 addendum). This
# reaper is the macOS last-resort BACKSTOP for browsers that still slip through
# (macOS has no PR_SET_PDEATHSIG / cgroup guarantee). It is deliberately NOT
# expanded into a count/aggregate cap — graceful teardown is the fix, not more
# bespoke heuristics.
#
# This reaper is the HOST-RESOURCE half of the mitigation (the test suite covers
# the recovery half). It periodically samples the RSS of Playwright's WebKit
# child processes and SIGKILLs any single one that exceeds a configurable cap —
# well above a healthy WebContent process, well below the level that exhausts the
# host. Killing a wedged WebContent child is safe: Playwright's retries:1
# fresh-context retry recovers the test, exactly as it already does for the hang.
#
# Sourced by scripts/lib/e2e.sh. Started by scripts/e2e-browser.sh; stopped on
# teardown (see setup_e2e_session's teardown_e2e and the umbrella e2e.sh).
#
# Knobs (all optional):
#   E2E_WEBKIT_RSS_CAP_MB     per-process RSS cap in MB        (default 6144)
#   E2E_WEBKIT_REAP_INTERVAL_S  sample interval in seconds     (default 5)
#   E2E_WEBKIT_REAP_MATCH     command substring a process must contain to be a
#                             candidate (default: the Playwright WebKit browsers
#                             cache path, e.g. "ms-playwright/webkit")
#   E2E_WEBKIT_REAP           set to 0/no/false to disable entirely
#   E2E_WEBKIT_REAPER_PIDFILE override the pidfile location (tests)

# In-memory handle to the running reaper loop (set by start_webkit_reaper).
WEBKIT_REAPER_PID="${WEBKIT_REAPER_PID:-}"

# ── config resolution ──────────────────────────────────────────────────
_reaper_cap_mb() { printf '%s' "${E2E_WEBKIT_RSS_CAP_MB:-6144}"; }

# Sample interval. A non-numeric (typo like "5s") or zero value would turn the
# loop into a `ps` busy-loop — `sleep` rejects it and `|| true` masks the error —
# so fall back to the 5s default rather than spin and hammer the host the reaper
# exists to protect.
_reaper_interval_s() {
    local v="${E2E_WEBKIT_REAP_INTERVAL_S:-5}"
    case "$v" in ''|*[!0-9]*|0) v=5 ;; esac
    printf '%s' "$v"
}

# The command-substring that marks a process as a Playwright WebKit child.
# Matching by the browsers-cache path (NOT a bare "WebContent" substring) is what
# keeps us off the user's own Safari, Chrome, the lucidos-engine, node/vite, and
# unrelated WebKit consumers. On macOS the WebContent/GPU/Networking XPC services
# launched by Playwright's WebKit all live under
# <browsers>/ms-playwright/webkit-NNNN/, so their argv path contains this token;
# Playwright's chromium lives under ms-playwright/chromium-NNNN/ and is excluded.
_reaper_match() {
    if [ -n "${E2E_WEBKIT_REAP_MATCH:-}" ]; then
        printf '%s' "$E2E_WEBKIT_REAP_MATCH"
    elif [ -n "${PLAYWRIGHT_BROWSERS_PATH:-}" ]; then
        printf '%s' "${PLAYWRIGHT_BROWSERS_PATH%/}/webkit"
    else
        # Default cache dir name is identical on macOS (~/Library/Caches) and
        # Linux (~/.cache); the "ms-playwright/webkit" suffix is what we key on.
        printf '%s' "ms-playwright/webkit"
    fi
}

_reaper_pidfile() {
    printf '%s' "${E2E_WEBKIT_REAPER_PIDFILE:-${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}/.lucidos/webkit-reaper.pid}"
}

# ── process sampling (the test seam) ───────────────────────────────────
# Emits one "PID RSS_KB COMMAND" line per process. Overridden by the test to feed
# synthetic rows for real sleeper PIDs without needing a browser. Kept tiny so the
# real implementation and the stub stay obviously equivalent.
#
# `-Aww` = every process, unlimited width (so long browser paths aren't
# truncated). RSS is in KB on both macOS and Linux.
_reaper_list_processes() {
    ps -Aww -o pid=,rss=,command= 2>/dev/null
}

# ── one sampling pass ──────────────────────────────────────────────────
# Kills every matched process whose RSS exceeds the cap. Errors (a dead PID, a
# permission failure, a missing ps) never abort the pass — this is a best-effort
# guard, not a correctness path.
reap_once() {
    local cap_mb cap_kb match self reaper_pid
    cap_mb=$(_reaper_cap_mb)
    case "$cap_mb" in ''|*[!0-9]*) cap_mb=6144 ;; esac
    cap_kb=$((cap_mb * 1024))
    match=$(_reaper_match)
    # The controlling script's PID. Inside the backgrounded loop this is the
    # parent e2e-browser.sh, not the loop — which is exactly what we want to
    # protect; the loop itself is covered by reaper_pid below.
    self=$$
    # The reaper loop's own PID. WEBKIT_REAPER_PID is set when reap_once runs in
    # the foreground (tests), but is EMPTY inside the backgrounded loop (it's
    # assigned after the fork). $BASHPID would give the loop its own PID but is
    # unavailable on macOS bash 3.2, so fall back to the pidfile the parent wrote.
    reaper_pid="${WEBKIT_REAPER_PID:-}"
    [ -z "$reaper_pid" ] && reaper_pid=$(cat "$(_reaper_pidfile)" 2>/dev/null)

    _reaper_list_processes | while read -r pid rss command; do
        # Skip header rows / garbage and anything that isn't plainly numeric.
        case "$pid" in ''|*[!0-9]*) continue ;; esac
        case "$rss" in ''|*[!0-9]*) continue ;; esac
        # Never touch init, our own controlling script, or the reaper loop.
        [ "$pid" -le 1 ] && continue
        [ "$pid" = "$self" ] && continue
        [ -n "$reaper_pid" ] && [ "$pid" = "$reaper_pid" ] && continue
        # Substring match on the full command path (see _reaper_match).
        case "$command" in
            *"$match"*) ;;
            *) continue ;;
        esac
        [ "$rss" -gt "$cap_kb" ] || continue
        # Backstop: never SIGKILL a protected host process (any workspace's live
        # engine/frontend). The path match already excludes them, so this is
        # defense-in-depth — the canary if the matcher ever broadens. Guarded by
        # command -v so the standalone unit test (which doesn't source ports.sh)
        # still runs.
        if command -v is_protected_host_pid >/dev/null 2>&1 && is_protected_host_pid "$pid"; then
            continue
        fi

        # SIGKILL, not SIGTERM: the target is a WEDGED WebContent process that
        # won't service a graceful signal, and the whole point is to free its RSS
        # now. Playwright's retries:1 fresh-context retry recovers the test.
        if kill -KILL "$pid" 2>/dev/null; then
            printf '[webkit-reaper] %s KILLED pid=%s rss=%sMB (cap=%sMB) cmd=%s\n' \
                "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$pid" "$((rss / 1024))" "$cap_mb" "$command"
        fi
    done
    return 0
}

# ── background loop ────────────────────────────────────────────────────
_reaper_loop() {
    local interval="$1"
    local sleep_pid=""
    # stop_webkit_reaper sends SIGTERM. Kill the in-flight `sleep` too, so the
    # loop leaves nothing reparented to init — honoring "never outlives the run".
    trap '[ -n "$sleep_pid" ] && kill "$sleep_pid" 2>/dev/null; exit 0' TERM INT
    while :; do
        reap_once || true
        sleep "$interval" &
        sleep_pid=$!
        wait "$sleep_pid" 2>/dev/null || true
        sleep_pid=""
    done
}

# ── lifecycle ──────────────────────────────────────────────────────────
# start_webkit_reaper — spawn the sampling loop in the background. Idempotent:
# a second call while a loop is alive is a no-op. Degrades gracefully when the
# platform can't sample (no `ps`) — warns and returns 0 so a Linux/CI run that
# lacks the expected tooling is never broken by the guard.
start_webkit_reaper() {
    case "${E2E_WEBKIT_REAP:-1}" in
        0|no|false|off)
            echo "[webkit-reaper] disabled via E2E_WEBKIT_REAP=${E2E_WEBKIT_REAP}"
            return 0
            ;;
    esac

    if [ -n "${WEBKIT_REAPER_PID:-}" ] && kill -0 "$WEBKIT_REAPER_PID" 2>/dev/null; then
        return 0
    fi

    if ! command -v ps >/dev/null 2>&1; then
        echo "[webkit-reaper] 'ps' unavailable — reaper not started (host-memory guard off)"
        return 0
    fi

    local cap_mb interval match
    cap_mb=$(_reaper_cap_mb)
    interval=$(_reaper_interval_s)
    match=$(_reaper_match)

    _reaper_loop "$interval" &
    WEBKIT_REAPER_PID=$!
    disown "$WEBKIT_REAPER_PID" 2>/dev/null || true

    local pidfile
    pidfile=$(_reaper_pidfile)
    mkdir -p "$(dirname "$pidfile")" 2>/dev/null || true
    echo "$WEBKIT_REAPER_PID" > "$pidfile" 2>/dev/null || true

    echo "[webkit-reaper] started (pid=$WEBKIT_REAPER_PID, cap=${cap_mb}MB, interval=${interval}s, match='$match')"
}

# stop_webkit_reaper — terminate the loop. Idempotent and safe in an EXIT trap:
# a no-op when nothing was started. Reads the pidfile as a fallback so the
# umbrella e2e.sh can reap a loop started by the e2e-browser.sh subprocess.
stop_webkit_reaper() {
    local pid pidfile
    pidfile=$(_reaper_pidfile)
    pid="${WEBKIT_REAPER_PID:-}"
    if [ -z "$pid" ] && [ -f "$pidfile" ]; then
        pid=$(cat "$pidfile" 2>/dev/null)
    fi

    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        # wait reaps it when it's our own child; harmless no-op cross-process.
        wait "$pid" 2>/dev/null || true
        echo "[webkit-reaper] stopped (pid=$pid)"
    fi

    rm -f "$pidfile" 2>/dev/null || true
    WEBKIT_REAPER_PID=""
}
