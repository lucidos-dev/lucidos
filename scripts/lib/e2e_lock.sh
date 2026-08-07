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
# tear down) leaves orphaned e2e processes alive, still holding their RSS. The
# nightly orchestrator re-spawned the full e2e suite THREE times on 2026-06-21,
# and each re-spawn reclaimed the "free" stale lock and stacked a fresh set of
# browsers on top of the orphans → 23.5 GB compressed + 14 GB swap, the machine
# pinned in critical memory pressure for 4+ hours.
#
# THREE KINDS of orphan, because a run leaks three kinds of process:
#   browser: Playwright's browser children, matched by the browsers-cache path.
#   engine:  the e2e-test workspace's own engine, keyed on its pidfile.
#   agent:   the CODING-AGENT subprocesses the suite's own tests spawn (Claude
#            Code / Codex, and the `lucidos mcp-permission-server` each one
#            runs). The engine starts them with their cwd inside a worktree
#            under the e2e workspace; when it dies they are re-parented to init
#            and keep running. Four survived 55 minutes on 2026-08-07, the
#            single largest contributor to a memory exhaustion that froze the
#            host, and nothing here looked for them: the sweep knew only about
#            browsers and the engine.
#
# The sweep runs at two moments, and it needs both:
#   - at TEARDOWN of every run that stops its workspace (`sweep_e2e_orphans`),
#     so a run cleans up after itself instead of banking on the next one; and
#   - before RECLAIMING a stale lock, the only backstop left when a run is
#     killed hard enough that its EXIT trap never fires.
# Reclaim re-scans after sweeping and takes the lock only once they are gone; if
# the sweep cannot clear them it REFUSES rather than stack. The four states:
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

# argv[0] basenames worth asking the kernel about for the `agent` kind. This
# list NARROWS the candidate set only; the cwd check below is what decides, so
# adding a name here can never by itself make something a kill candidate.
# `lucidos` is the engine-bundled CLI that Claude Code runs as its MCP
# permission server. `lucidos-engine` is deliberately absent: the engine has its
# own pidfile-keyed kind, and listing it here would double-report it.
_e2e_orphan_agent_basenames() {
    printf '%s\n' claude codex node lucidos
}

# A pid's current working directory, or empty when it cannot be read.
#
# This is a KERNEL FACT, which is the whole reason the `agent` kind keys on it.
# argv[0] cannot separate an e2e coding-agent subprocess from the user's own
# session (both are `.../bin/claude`), and the rest of the command line is
# actively unsafe to match: a Claude Code process carries the engine's thread
# history inside a ~22 KB `--append-system-prompt`, so a session that merely
# DISCUSSES the e2e workspace's paths contains them verbatim (the session that
# designed this sweep did). That is not hypothetical: the sibling matcher in
# webkit_reaper.sh SIGKILLed two real sessions on 2026-08-03 for quoting a path.
# No prompt text can forge a cwd.
#
# Test seam: overridden by the test to feed synthetic paths.
_e2e_proc_cwd() {
    local pid="$1"
    if [ -r "/proc/$pid/cwd" ]; then
        readlink "/proc/$pid/cwd" 2>/dev/null
        return 0
    fi
    lsof -a -d cwd -p "$pid" -Fn 2>/dev/null | sed -n 's/^n//p' | head -1
}

# The e2e workspace dir with symlinks resolved, so a prefix test can match the
# real paths `lsof` reports. Falls back to the unresolved form if it is gone.
_e2e_resolved_workspace_dir() {
    local ws
    ws="$(_e2e_workspace_dir)"
    (cd "$ws" 2>/dev/null && pwd -P) || printf '%s' "$ws"
}

# Is $1 the directory $2, or anything beneath it? Prefix test on whole path
# components, so a sibling like `<ws>-old` can never match `<ws>`.
#
# Trailing slashes are stripped from the root first. `_e2e_workspace_dir` hands
# back $E2E_WORKSPACE verbatim, so an operator who exported it with one would
# otherwise make `<root>/*` read `<root>//*`, which matches nothing: the scan
# would go silently blind, the same class of disarming as the whitespace
# browsers path that `_e2e_list_orphans` warns about.
_e2e_path_under() {
    local path="$1" root="$2"
    while [ "${root%/}" != "$root" ] && [ -n "${root%/}" ]; do root="${root%/}"; done
    [ -n "$path" ] && [ -n "$root" ] || return 1
    case "$path" in
        "$root" | "$root"/*) return 0 ;;
    esac
    return 1
}

# Is $1 an ancestor of this shell? Never signal one (ADR 0025): a sweep that can
# reach its own caller kills the run that is trying to clean up. The cwd gate
# already excludes our own session, so this is defence in depth, and it is the
# same posture `is_protected_host_pid` takes in ports.sh. Bounded walk so a
# malformed ppid chain cannot spin.
_e2e_is_ancestor_of_self() {
    local target="$1" p=$$ hops=0
    while [ "$p" -gt 1 ] && [ "$hops" -lt 64 ]; do
        [ "$p" = "$target" ] && return 0
        p="$(ps -o ppid= -p "$p" 2>/dev/null | tr -d ' ')"
        case "$p" in ''|*[!0-9]*) return 1 ;; esac
        hops=$((hops + 1))
    done
    return 1
}

# Emit "KIND PID" for every LIVE orphan of a prior e2e run. KIND ∈ browser|engine|agent.
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

    # Coding-agent subprocesses the suite's own tests spawned. Two-stage on
    # purpose: argv[0]'s basename narrows the candidates (so we ask the kernel
    # about a handful of pids, not 500), and the CWD decides. See `_e2e_proc_cwd`
    # for why nothing else in the command line may be trusted here.
    #
    # No ppid==1 gate. Both callers hold the lock, so nothing else is legitimately
    # driving this workspace: at teardown the engine has already been stopped, and
    # on the reclaim path the owning run is dead. Requiring re-parenting would
    # also miss an agent whose parent is another leaked agent.
    # Both forms of the workspace root, because the two sides of the comparison
    # are resolved differently: `lsof` reports a cwd with symlinks already
    # resolved, while $E2E_WORKSPACE may be written through one (macOS `/var` is
    # a symlink to `/private/var`, which is where a temp-dir workspace lives).
    # Matching either root means a symlinked path can't silently disarm the scan.
    local ws_raw ws_real agent_names argv0 cwd name
    ws_raw="$(_e2e_workspace_dir)"
    ws_real="$(_e2e_resolved_workspace_dir)"
    agent_names="$(_e2e_orphan_agent_basenames)"
    _e2e_orphan_ps | while read -r pid command; do
        case "$pid" in ''|*[!0-9]*) continue ;; esac
        [ "$pid" -le 1 ] && continue
        [ "$pid" = "$self" ] && continue
        argv0="${command%% *}"
        local matched=""
        while IFS= read -r name; do
            [ -z "$name" ] && continue
            [ "${argv0##*/}" = "$name" ] && { matched=1; break; }
        done <<EOF
$agent_names
EOF
        [ -n "$matched" ] || continue
        cwd="$(_e2e_proc_cwd "$pid")"
        _e2e_path_under "$cwd" "$ws_real" || _e2e_path_under "$cwd" "$ws_raw" || continue
        _e2e_is_ancestor_of_self "$pid" && continue
        echo "agent $pid"
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
    local agents=""
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
            agent)
                # SIGTERM first: a coding agent owns a git worktree and a child
                # MCP server, and its own handler unwinds both far more tidily
                # than we can. Escalated below rather than trusted, because one
                # that ignores it keeps ~150 MB and a node runtime for the life
                # of the host, which is the pile-up being prevented.
                kill -TERM "$pid" 2>/dev/null \
                    && echo "[e2e-lock] asked orphan agent pid=$pid to stop (SIGTERM)" >&2
                agents="$agents $pid"
                ;;
        esac
    done
    if [ -n "$agents" ]; then
        sleep "${E2E_ORPHAN_AGENT_GRACE_S:-2}"
        for pid in $agents; do
            kill -0 "$pid" 2>/dev/null || continue
            kill -KILL "$pid" 2>/dev/null \
                && echo "[e2e-lock] orphan agent pid=$pid ignored SIGTERM, killed" >&2
        done
    fi
}

# ── sweep_e2e_orphans ───────────────────────────────────────────────────
# Reap whatever this run left behind. Called from the teardown chain AFTER the
# workspace is stopped, so the agents the tests spawned have already lost their
# parent and nothing legitimate is still driving the workspace.
#
# This is the half that makes cleanup UNCONDITIONAL. The reclaim path above only
# ever runs when the NEXT run finds a stale lock, so a run that finished cleanly,
# or one whose successor never came, left its agents alive indefinitely: that is
# exactly how four of them reached 55 minutes on 2026-08-07.
#
# Never fails the caller and never blocks it for long: teardown must not turn a
# green run red, and an orphan that survives is still caught by the reclaim path.
sweep_e2e_orphans() {
    local orphans
    orphans="$(_e2e_list_orphans 2>/dev/null)" || return 0
    [ -n "$orphans" ] || return 0
    echo "[e2e-lock] teardown: sweeping processes this run left behind:" >&2
    printf '%s\n' "$orphans" | sed 's/^/[e2e-lock]   orphan: /' >&2
    _e2e_reap_orphans <<EOF
$orphans
EOF
    return 0
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
            # an agent gets a SIGTERM grace before its SIGKILL; the engine's
            # SIGUSR1 graceful shutdown can take up to its ~10s budget.
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
                echo "and any leftover browser or coding-agent PIDs with: kill -KILL <pid>" >&2
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
