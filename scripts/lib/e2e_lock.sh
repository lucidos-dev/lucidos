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
#   STARTED_EPOCH=<the same instant in epoch seconds>
#   SCRIPT=<entry-point name>
#
# STARTED_EPOCH is redundant with STARTED and exists so held_secs is portable
# arithmetic: parsing the ISO form back needs `date -j -f` on BSD and `date -d`
# on GNU. An unknown key is ignored by the reader, so a lock file written before
# it existed still reclaims; its release just carries no held_secs.
#
# THE LOCK ANNOUNCES ITSELF. Every hold emits E2ELockAcquired when it starts and
# E2ELockReleased when it ends, as domain events through `lucidos events emit`,
# so a run that LOST the lock can subscribe with `lucidos await-event` and end
# its turn instead of busy-waiting. On 2026-08-09 three coding-agent threads
# raced for this lock and both losers hand-rolled a sleep loop, one of them a
# 40 minute foreground tool call re-executing the entry script every 20 seconds.
# The refusal below teaches the subscribe path, and .claude/skills/e2e-lock-wait/
# carries the full rules. Two gaps are accepted rather than closed, and both
# recover through the subscriber's own timeout:
#   - CROSS-WORKSPACE. The lock is shared by every workspace on the machine, but
#     an emit lands in the emitting subprocess's own $LUCIDOS_WORKSPACE, so a
#     holder in workspace A never wakes a waiter in workspace B. The refusal says
#     so when it can tell (`_e2e_holder_is_another_workspace`).
#   - ENGINE DOWN AT RELEASE. The emit is an HTTP POST to a live engine and is
#     best effort, so nothing is written and there is nothing for the waiter's
#     boot catch-up scan to find. Narrow in practice: a wake only works
#     same-workspace, so the waiter shares that engine and is down with it.
# A holder killed hard enough to skip its EXIT trap IS covered: the next run to
# reclaim its stale lock emits the release on its behalf, outcome=reclaimed.
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

# Fields of the lock file, as filled in by `_e2e_read_lock_file`. Globals rather
# than a return value because bash cannot return a record, and both the acquire
# and the release path need the same set. Initialised here so a caller running
# under `set -u` is safe before the first read.
_E2E_LK_PID=""
_E2E_LK_THREAD=""
_E2E_LK_WORKTREE=""
_E2E_LK_STARTED=""
_E2E_LK_STARTED_EPOCH=""
_E2E_LK_SCRIPT=""

# PID of the announcement `acquire_e2e_lock` backgrounded, so the release can
# order itself after it. See `_e2e_await_acquire_announcement`.
E2E_LOCK_ANNOUNCE_PID=""

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

# ── the lock file ───────────────────────────────────────────────────────

# Read every field of a lock file into the _E2E_LK_* globals. Returns non-zero
# when there is no file to read.
#
# IFS='=' keeps everything after the FIRST '=' in $val, so a worktree path
# containing '=' survives. Every field is cleared first, so a second call over a
# shorter file cannot leave the previous one's values standing.
#
# `|| [ -n "$key" ]` catches a file whose LAST line has no trailing newline:
# `read` returns non-zero there having already filled the variables, so the plain
# form silently drops that line. Our own writer uses a heredoc and always ends
# with one, but SCRIPT is written last, so a hand-written or truncated lock file
# would lose exactly the field the refusal names.
_e2e_read_lock_file() {
    local file="$1" key val
    _E2E_LK_PID=""
    _E2E_LK_THREAD=""
    _E2E_LK_WORKTREE=""
    _E2E_LK_STARTED=""
    _E2E_LK_STARTED_EPOCH=""
    _E2E_LK_SCRIPT=""
    [ -f "$file" ] || return 1
    while IFS='=' read -r key val || [ -n "$key" ]; do
        case "$key" in
            PID)           _E2E_LK_PID="$val" ;;
            THREAD_ID)     _E2E_LK_THREAD="$val" ;;
            WORKTREE)      _E2E_LK_WORKTREE="$val" ;;
            STARTED)       _E2E_LK_STARTED="$val" ;;
            STARTED_EPOCH) _E2E_LK_STARTED_EPOCH="$val" ;;
            SCRIPT)        _E2E_LK_SCRIPT="$val" ;;
        esac
    done < "$file"
    return 0
}

# Atomic create-or-fail using noclobber. Returns 0 on success, non-zero if file exists.
_e2e_lock_write() {
    local lock_file="$1" script_name="$2"
    local thread_id="${LUCIDOS_THREAD_ID:-unknown}"
    local started started_epoch
    started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    started_epoch="$(date +%s)"
    (set -C; cat > "$lock_file" <<EOF
PID=$$
THREAD_ID=$thread_id
WORKTREE=$PWD
STARTED=$started
STARTED_EPOCH=$started_epoch
SCRIPT=$script_name
EOF
    ) 2>/dev/null
}

# ── announcing a hold (the E2ELock* domain events) ──────────────────────
# See the header. These let a refused run subscribe instead of poll. Every one
# of them is best effort: an e2e run must never go red, and an EXIT trap must
# never stall, because the engine was briefly unreachable.
#
# SYNCHRONOUS FROM THE TRAP, BACKGROUNDED FROM ACQUIRE, and the asymmetry is
# load-bearing rather than a style choice. `release_e2e_lock` runs inside the
# caller's EXIT trap with the lock already gone: nothing there is worth
# protecting, so it emits in the foreground and its bound exists only to stop
# teardown stalling. `acquire_e2e_lock` is the opposite: it returns having TAKEN
# the lock, and both entry points install their teardown only afterwards
# (`scripts/e2e.sh`, `setup_e2e_session`), so every millisecond it blocks widens
# the window in which an interrupt leaves a stale lock nobody releases. That
# window was ~20ms of `kill_orphan_simulator`; a foreground emit against a
# WEDGED engine would have made it 10s (the reclaim path announces twice). So
# acquire's announcements are backgrounded at the call site. The child bounds
# and kills itself exactly as it would in the foreground, so a shell that exits
# first leaves nothing unbounded behind.

# Escape a string for embedding in a JSON string literal: backslash and double
# quote, plus control characters dropped (a JSON string may not carry them raw).
#
# Needed here where `release_events.sh` gets away without it: that library embeds
# fixed step ids and N.N.N versions, while WORKTREE is `$PWD` at acquire time and
# is therefore arbitrary. Backslash is escaped before quote, so an escaped quote
# is not re-escaped.
_e2e_json_escape() {
    printf '%s' "$1" | tr -d '[:cntrl:]' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

# The JSON both lock events share: which entry point, which thread, which
# worktree. $4 is a pre-built fragment of per-event fields, each leading with a
# comma, or empty.
_e2e_lock_event_payload() {
    local script="$1" thread="$2" worktree="$3" extra="$4"
    printf '{"script":"%s","thread_id":"%s","worktree":"%s"%s}' \
        "$(_e2e_json_escape "$script")" \
        "$(_e2e_json_escape "$thread")" \
        "$(_e2e_json_escape "$worktree")" \
        "$extra"
}

# Whole seconds a hold lasted, or EMPTY when it cannot be known: a lock file
# written before STARTED_EPOCH existed, a value that is not a plain number, or a
# clock that went backwards. Empty means the caller omits the field rather than
# emitting `"held_secs":` with nothing after it, which would be invalid JSON.
#
# ALWAYS exits 0, including on every one of those. Callers read it through a
# command substitution in an EXIT trap under `set -e`, where a non-zero exit
# would take the teardown with it.
_e2e_lock_held_secs() {
    local started="$1" now
    case "$started" in ''|*[!0-9]*) return 0 ;; esac
    # Digits alone are not enough: bash's `[` errors with "integer expression
    # expected" on a value outside intmax, and that line would surface mid
    # teardown over a lock file we have already decided we cannot read. 11
    # digits reaches the year 5138.
    [ "${#started}" -le 11 ] || return 0
    now="$(date +%s)"
    [ "$now" -ge "$started" ] || return 0
    printf '%s' "$(( now - started ))"
}

# Emit one lock event, bounded and best effort. Always returns 0.
#
# The guard is $E2E_LOCK_DIR_OVERRIDE. This library's own suite exports it for
# the whole file to sandbox the lock path (scripts/lib/e2e_lock_test.sh), and
# nothing else sets it, so keying on it is what stops a unit-test run from
# writing E2ELock* events into the developer's live workspace. The suite's emit
# cases drop it inside a subshell and stay sandboxed through $E2E_WORKSPACE,
# which it pins for the same reason.
#
# Bounded in the shell rather than left to the CLI's own 30s reqwest default
# (crates/lucidos-cli/src/http.rs): this runs inside an EXIT trap, where half a
# minute of teardown stall is not acceptable, and that default does not cover a
# `lucidos` wedged before its HTTP client exists. macOS ships no `timeout`
# binary, hence the tick loop.
_e2e_emit_lock_event() {
    local event="$1" summary="$2" payload="$3"
    [ -z "${E2E_LOCK_DIR_OVERRIDE:-}" ] || return 0
    command -v lucidos >/dev/null 2>&1 || return 0

    local timeout_s="${E2E_LOCK_EMIT_TIMEOUT_S:-5}"
    case "$timeout_s" in ''|*[!0-9]*) timeout_s=5 ;; esac

    lucidos events emit "$event" --summary "$summary" --payload "$payload" \
        >/dev/null 2>&1 &
    local pid=$!
    # A WALL-CLOCK deadline, not a tick count. Counting `sleep 0.1` iterations
    # bounds the number of naps, not the elapsed time: each one costs a fork,
    # ~0.25s on a busy Mac rather than 0.10s, so a 5s "bound" ran 12s and drifted
    # furthest exactly when the host is loaded, which is when an e2e teardown
    # happens. `SECONDS` is a bash builtin, so reading it costs no fork of its
    # own (unlike the `date` in the orphan reap loop above). Its 1s granularity
    # makes the true ceiling `timeout_s` plus one poll, and the floor
    # `timeout_s - 1`, which at the 5s default is nowhere near a healthy emit
    # against a local engine (~0.2s).
    local deadline=$(( SECONDS + timeout_s ))
    while [ "$SECONDS" -lt "$deadline" ] && kill -0 "$pid" 2>/dev/null; do
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
        # Only ever the child backgrounded three lines up, so the
        # never-signal-an-ancestor rule (ADR 0025) cannot be reached from here.
        kill -KILL "$pid" 2>/dev/null || :
        echo "[e2e-lock] $event emit exceeded ${timeout_s}s and was abandoned" >&2
    fi
    wait "$pid" 2>/dev/null || :
    return 0
}

# A hold has started. `$2` is true when this run took the lock over from a dead
# owner rather than finding it free.
_e2e_announce_lock_acquired() {
    local script="$1" reclaimed="$2"
    local summary="e2e lock acquired by $script"
    if [ "$reclaimed" = true ]; then
        summary="$summary (reclaimed from a dead owner)"
    fi
    _e2e_emit_lock_event E2ELockAcquired "$summary" \
        "$(_e2e_lock_event_payload "$script" "${LUCIDOS_THREAD_ID:-unknown}" \
            "$PWD" ",\"reclaimed\":$reclaimed")"
}

# A hold has ended, and this is the event a blocked run waits on.
#
# `outcome` is `released` (the owner's own EXIT trap) or `reclaimed` (a dead
# owner's lock taken over by a later run). A waiter cares about both: it is
# blocked on the hold, and a hold whose owner died is over just as finally.
# Every argument describes the hold that ENDED, which on the reclaim path is the
# dead owner's rather than the caller's.
_e2e_announce_lock_released() {
    local script="${1:-unknown}" thread="${2:-unknown}" worktree="${3:-unknown}"
    local started_epoch="$4" outcome="$5"
    local held extra summary
    held="$(_e2e_lock_held_secs "$started_epoch")"
    extra=",\"outcome\":\"$outcome\""
    summary="e2e lock $outcome by $script"
    if [ -n "$held" ]; then
        extra="$extra,\"held_secs\":$held"
        summary="$summary after ${held}s"
    fi
    _e2e_emit_lock_event E2ELockReleased "$summary" \
        "$(_e2e_lock_event_payload "$script" "$thread" "$worktree" "$extra")"
}

# Let the announcement `acquire_e2e_lock` backgrounded finish before this run
# announces anything else, so the two cannot be persisted out of order.
#
# Without it a short-lived run can emit `E2ELockReleased` (synchronous) before
# its own `E2ELockAcquired` (backgrounded) has landed, and anyone replaying the
# timeline reads a lock that was released and then taken and never given back.
# The wake path does not care (a waiter watches only for a release), but a
# timeline that lies is the entire reason the acquire event exists at all.
#
# Blocking HERE is fine and blocking in acquire was not: this runs inside the
# caller's EXIT trap with the lock already gone. Bounded by that child's own
# emit timeout, and `wait` names one pid rather than taking every background job
# of the entry script, which would otherwise sit on the webkit reaper and the
# host-load sampler until the run ended.
_e2e_await_acquire_announcement() {
    [ -n "${E2E_LOCK_ANNOUNCE_PID:-}" ] || return 0
    local pid="$E2E_LOCK_ANNOUNCE_PID"
    E2E_LOCK_ANNOUNCE_PID=""
    # Already reaped (the test suite waits on its own children) reports "not a
    # child of this shell", which is a success for our purposes: it is done.
    wait "$pid" 2>/dev/null || :
    return 0
}

# Would a release by this holder wake THIS session? No, when the holder is
# working out of a different workspace: `lucidos events emit` writes to the
# emitting subprocess's own $LUCIDOS_WORKSPACE while the lock is shared across
# every workspace on the machine, so that release lands in an event store this
# thread does not watch.
#
# Answers "no" (non-zero) whenever it cannot tell, so the refusal's note appears
# only when the gap is certain. Both forms of our own root are tried, because
# the holder's WORKTREE is a `$PWD` with symlinks already resolved while
# $LUCIDOS_WORKSPACE may be written through one: the same double comparison
# `_e2e_list_orphans` makes, for the same reason.
_e2e_holder_is_another_workspace() {
    local holder="$1" ws="${LUCIDOS_WORKSPACE:-}" ws_real
    [ -n "$holder" ] && [ -n "$ws" ] || return 1
    if _e2e_path_under "$holder" "$ws"; then
        return 1
    fi
    ws_real="$(cd "$ws" 2>/dev/null && pwd -P)" || ws_real=""
    if [ -n "$ws_real" ] && _e2e_path_under "$holder" "$ws_real"; then
        return 1
    fi
    return 0
}

# acquire_e2e_lock <script-name>
# Returns 0 on success, 1 on conflict.
acquire_e2e_lock() {
    local script_name="${1:-e2e}"
    local lock_file
    lock_file="$(_e2e_lock_path)"

    if _e2e_lock_write "$lock_file" "$script_name"; then
        E2E_LOCK_OWNED="$lock_file"
        # Backgrounded: the lock is now HELD and the caller has not armed its
        # teardown yet. See the section header above. The pid is kept so the
        # release can order itself after this, rather than racing it.
        _e2e_announce_lock_acquired "$script_name" false &
        E2E_LOCK_ANNOUNCE_PID=$!
        return 0
    fi

    # File exists: read all metadata in one pass. The read can still fail if the
    # holder released in the window since the write above. `|| :` is defensive
    # rather than load-bearing today: a bare non-zero call takes `set -e` with
    # it, but both entry points invoke this as `acquire_e2e_lock <label> ||
    # exit 1`, and a `||` list suppresses errexit inside the function. It is here
    # so a future bare caller cannot turn that refusal into a silent exit. The
    # same is NOT true of `release_e2e_lock`, which runs bare from an EXIT trap.
    _e2e_read_lock_file "$lock_file" || :
    local existing_pid="$_E2E_LK_PID" existing_thread="$_E2E_LK_THREAD"
    local existing_wt="$_E2E_LK_WORKTREE" existing_started="$_E2E_LK_STARTED"
    local existing_started_epoch="$_E2E_LK_STARTED_EPOCH"
    local existing_script="$_E2E_LK_SCRIPT"

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
            # The dead owner's hold is over, and a waiter blocked on that hold
            # is blocked on exactly this. Announced AFTER the new lock file is
            # written, so a waiter that wakes and retries reads the true state
            # (held by us) rather than a gap that is about to close.
            #
            # Backgrounded, both of them, for the reason in the section header:
            # we are holding the lock with no teardown armed. This is the path
            # that made it matter, since it announces twice.
            # ONE background child running both in sequence, not two racing
            # children: the dead owner's release must be persisted before this
            # run's acquire, or the timeline shows the lock taken and then
            # released by someone else.
            {
                _e2e_announce_lock_released "${existing_script:-unknown}" \
                    "${existing_thread:-unknown}" "${existing_wt:-unknown}" \
                    "$existing_started_epoch" reclaimed
                _e2e_announce_lock_acquired "$script_name" true
            } &
            E2E_LOCK_ANNOUNCE_PID=$!
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
    echo "Playwright browser processes: two sessions on a 32 GB Mac OOM'd the"
    echo "system on 2026-04-19."
    echo ""
    echo "Do NOT sleep, poll, or write a retry loop. Subscribe and end your turn:"
    echo "  lucidos await-event --on E2ELockReleased --timeout-secs 21600 \\"
    echo "    --reason \"waiting for the e2e lock\""
    echo "The engine re-opens this thread when the holder releases; retry once then."
    echo "Full rules: .claude/skills/e2e-lock-wait/SKILL.md"
    if _e2e_holder_is_another_workspace "$existing_wt"; then
        echo "NOTE: that holder runs in another workspace, so its release event will"
        echo "      not reach this thread. The timeout is your only wake here."
    fi
    echo ""
    echo "Lock file: $lock_file"
    return 1
}

# release_e2e_lock: only removes the lock if we own it. Safe in EXIT traps.
#
# The E2ELockReleased emit comes AFTER the file is gone. A waiter woken by it
# retries at once, so waking one to find the lock still held would spend one of
# its ten consecutive subscriptions on nothing.
release_e2e_lock() {
    local owned="${E2E_LOCK_OWNED:-}"
    # Cleared up front: this shell is done with the lock either way, and the
    # early returns below would otherwise each have to remember to clear it.
    E2E_LOCK_OWNED=""
    [ -n "$owned" ] || return 0
    [ -f "$owned" ] || return 0
    # `|| :` because a bare non-zero call takes `set -e` with it, and this runs
    # inside an EXIT trap where that truncates the rest of teardown.
    _e2e_read_lock_file "$owned" || :

    # "Only if we own it" used to mean only that this shell had set
    # E2E_LOCK_OWNED, so a lock some other run had legitimately reclaimed (our
    # pid unreachable to its `kill -0`, e.g. a different user on the host) was
    # deleted out from under it on our way past. Skip ONLY on positive evidence
    # of another owner: an absent or unreadable pid still gets removed, because
    # a lock file nobody will ever clean up wedges every future run, which is
    # the worse of the two failures.
    if [ -n "$_E2E_LK_PID" ] && [ "$_E2E_LK_PID" != "$$" ]; then
        echo "[e2e-lock] not releasing $owned: it is held by PID $_E2E_LK_PID now, not us ($$)" >&2
        return 0
    fi

    local script="${_E2E_LK_SCRIPT:-unknown}" thread="${_E2E_LK_THREAD:-unknown}"
    local worktree="${_E2E_LK_WORKTREE:-unknown}"
    local started_epoch="$_E2E_LK_STARTED_EPOCH"

    # The announcement is conditional on the removal actually happening. `rm -f`
    # is silent about a missing file but NOT about a permission or filesystem
    # error, and announcing through one would wake every waiter onto a lock that
    # is still held: they retry, are refused, and each spends one of its ten
    # consecutive subscriptions on a release that never happened. Tested in a
    # condition rather than run bare for the reason the read above is: this is an
    # EXIT trap under `set -e`.
    #
    # The reclaim path needs no equivalent, because its `rm` is followed by a
    # noclobber `_e2e_lock_write` that fails in exactly the same case, and its
    # announcement is already gated on that write succeeding.
    if ! rm -f "$owned"; then
        echo "[e2e-lock] WARNING: could not remove $owned, so the lock is STILL HELD." >&2
        echo "[e2e-lock] Not announcing a release: a waiter woken by it would find" >&2
        echo "[e2e-lock] the lock taken and would have burned a subscription." >&2
        return 0
    fi
    _e2e_await_acquire_announcement
    _e2e_announce_lock_released "$script" "$thread" "$worktree" \
        "$started_epoch" released
}
