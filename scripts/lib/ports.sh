#!/bin/bash
# Port allocation for multi-workspace support.
#
# Each workspace gets a stable port offset stored in ~/.lucidos/port-registry.
# The engine port is LUCIDOS_ENGINE_PORT_BASE + offset. The shared Postgres
# container's first guess is LUCIDOS_SHARED_PG_PORT, one port for every
# workspace. Both sit off the defaults of common dev tools (ADR 0821).
#
# Precedence for the desired vite port:
#   1. LUCIDOS_VITE_PORT env var  (highest — for one-off "I need a specific port")
#   2. <workspace>/lucidos.toml   [ports] vite = N  (per-workspace pin)
#   3. registry entry             (stable offset assigned on first run)
#   4. next-free offset           (max(registry) + 1, or 0 if empty)
#
# On collision (the desired engine port is occupied by something that isn't
# ours), walk the offset forward by 1 until it is free. The walked offset is
# persisted in the registry.
#
# A pin in the legacy range moves to the same offset on the new base, and the
# launch says so (see translate_legacy_engine_port).
#
# Exports: API_PORT and VITE_PORT (both the engine port), PG_PORT.
#
# The whole allocation is one critical section under an exclusive flock on
# <registry>.lock: lookup, next offset, the walk and the save. So two launches
# can never claim one offset (ADR 0438).

# shellcheck source=scripts/lib/flock.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/flock.sh"

# shellcheck source=scripts/lib/storage_root.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/storage_root.sh"
# shellcheck source=scripts/lib/e2e_root.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/e2e_root.sh"

LUCIDOS_PORT_REGISTRY="$(lucidos_home_dir)/port-registry"
# The two engine bases and this limit have Rust copies in lucidos-gateway-constants,
# pinned to these lines by a test there.
LUCIDOS_PORT_WALK_LIMIT=1000
# Below 32768, Linux's ephemeral range, and off every common tool default.
LUCIDOS_ENGINE_PORT_BASE=17173
# Where dev engines lived before: Vite's own default. Read only to move a
# workspace off it.
LUCIDOS_LEGACY_ENGINE_PORT_BASE=5173
LUCIDOS_SHARED_PG_PORT=19432
# The descriptor allocate_ports holds the registry lock on.
PORT_REGISTRY_LOCK_FD=7

# ── ports_file_set ──────────────────────────────────────────────────────
# Set KEY=VALUE lines in a workspace's ports file, preserving every other line.
#
# The shell twin of merge_ports_file in crates/lucidos-gateway/src/stack.rs, and
# it exists for the same reason: this file has several owners. allocate_ports
# writes the two port keys and swap_ports the Postgres keys. detect_tls writes
# PROTO, and adopt_engine_scheme or record_engine_scheme correct it from the
# live engine. The gateway republishes API_PORT and PROTO on every spawn and
# re-adoption. scripts/status.sh sources the result.
#
# A truncating write is what broke that. _finalize_ports used to `cat >` the
# file with the two port keys, so any launch path that allocated ports without
# going on to detect_tls left no PROTO line behind. read_ports then defaults an
# absent PROTO to https, and a cross-workspace call into a plain-http engine
# dies on a TLS handshake with nothing naming the cause.
#
# Setting an existing key rewrites it IN PLACE, so repeated launches cannot
# stack duplicate lines the way an append did.
ports_file_set() {
    local file="$1"; shift
    mkdir -p "$(dirname "$file")"
    # An `if` rather than `[ -f … ] && …`: the entry scripts run under `set -e`,
    # where an and-list whose left side fails takes the whole list's status.
    local existing=""
    if [ -f "$file" ]; then
        existing="$(cat "$file")"
    fi
    local out="" line pair key replaced written=" "
    while IFS= read -r line; do
        # A blank line is dropped rather than carried: an empty $existing reads
        # as one, and the format has none anyway.
        [ -n "$line" ] || continue
        replaced=""
        for pair in "$@"; do
            key="${pair%%=*}"
            if [ "${line#*=}" != "$line" ] && [ "${line%%=*}" = "$key" ]; then
                # The FIRST occurrence becomes the new value and every later one
                # is dropped. A file that accumulated duplicates under the old
                # appending writer is collapsed here, which is the whole repair:
                # rewriting each of them in place would keep all of them.
                case "$written" in
                    *" $key "*) ;;
                    *)
                        out+="$pair"$'\n'
                        written+="$key "
                        ;;
                esac
                replaced=1
                break
            fi
        done
        [ -n "$replaced" ] || out+="$line"$'\n'
    done <<< "$existing"
    # Whatever the file did not already carry goes on the end, in the order the
    # caller gave. Tracked by name rather than re-scanned, so a VALUE that looks
    # like another key cannot answer for one.
    for pair in "$@"; do
        key="${pair%%=*}"
        case "$written" in *" $key "*) continue ;; esac
        out+="$pair"$'\n'
    done
    printf '%s' "$out" > "$file"
}

# Check if a port is available for binding. Only LISTEN sockets prevent a
# fresh bind() — outbound client sockets that happen to have a remote port
# matching `port` (e.g. a stale browser tab still pointing at our previous
# engine on this port) show up in `lsof -i :PORT` but don't conflict with
# binding. Without `-sTCP:LISTEN`, those stale client connections cause
# allocate_ports to walk past a workspace's pinned port.
port_is_free() {
    local port="$1"
    ! lsof -ti :"$port" -sTCP:LISTEN >/dev/null 2>&1
}

# This process and every ancestor of it, space-padded (" 123 456 ") so a
# `case` membership test can't match a substring of a pid. Computed lazily,
# once; a process tree doesn't change above you.
_LUCIDOS_ANCESTOR_PIDS=""

# Populate _LUCIDOS_ANCESTOR_PIDS. Assigns the global DIRECTLY instead of
# echoing it: a `$(...)` caller would run this in a subshell and the cache
# would be discarded every single time.
_ensure_ancestor_pid_set() {
    [ -n "$_LUCIDOS_ANCESTOR_PIDS" ] && return 0
    # $$ — not $BASHPID, which macOS's bash 3.2 doesn't have — is the shell
    # that sourced us, and it stays stable inside subshells, so every caller
    # computes the same set.
    local acc=" $$ " cur="$$" steps=0
    # 64 is far past any real chain (the deepest here is ~7: test script →
    # bash → claude → engine → supervisor → gateway → bash → launchd) and
    # bounds the cost at one `ps` fork per level, once per process.
    while [ "$steps" -lt 64 ]; do
        cur=$(ps -o ppid= -p "$cur" 2>/dev/null | tr -d '[:space:]')
        # No parent, unreadable, or we walked off the top.
        case "$cur" in ''|*[!0-9]*) break ;; esac
        [ "$cur" -le 1 ] && break
        # A repeat means `ps` gave us a cycle. Impossible in a real process
        # tree, but a cheap guard against spinning if it ever lies.
        case "$acc" in *" $cur "*) break ;; esac
        acc="$acc$cur "
        steps=$(( steps + 1 ))
    done
    _LUCIDOS_ANCESTOR_PIDS="$acc"
}

# The account's home as recorded in the PASSWORD DATABASE, which $HOME cannot
# override. Empty when it can't be resolved or is the same as $HOME (so the
# pidfile scan doesn't walk the same tree twice).
_LUCIDOS_PASSWD_HOME=""
_LUCIDOS_PASSWD_HOME_RESOLVED=""
_ensure_passwd_home() {
    [ -n "$_LUCIDOS_PASSWD_HOME_RESOLVED" ] && return 0
    _LUCIDOS_PASSWD_HOME_RESOLVED=1
    local user home=""
    user=$(id -un 2>/dev/null) || return 0
    [ -n "$user" ] || return 0
    # Directory services first (macOS), then getent (Linux). Whichever isn't
    # installed just yields nothing. Deliberately NOT `eval "echo ~$user"` —
    # that would interpolate a username straight into shell code.
    #
    # `sed -n 's/^KEY: //p'`, not `awk '{print $2}'`: the value is the rest of
    # the line, so a home directory containing a space would be TRUNCATED by a
    # field split, and the scan would then walk a path that doesn't exist —
    # the arm would silently stop protecting, which is the failure mode this
    # whole change exists to remove.
    home=$(dscl . -read "/Users/$user" NFSHomeDirectory 2>/dev/null | sed -n 's/^NFSHomeDirectory: //p')
    [ -n "$home" ] || home=$(getent passwd "$user" 2>/dev/null | cut -d: -f6)
    # Must be absolute. Anything else (empty, an unexpected `dscl` line shape)
    # means we misparsed — discard it rather than scan a bogus root.
    case "$home" in /*) ;; *) return 0 ;; esac
    [ "$home" = "${HOME:-}" ] && return 0
    _LUCIDOS_PASSWD_HOME="$home"
}

# Return 0 (true) if `pid` belongs to a live Lucidos host process we must
# never signal: the engine that spawned us, its build-watch, or any other
# workspace's engine.
#
# FOUR arms. The first two read caller-owned state and a caller can switch
# them off; the last two cannot, which is the whole point.
#
#  1. LUCIDOS_HOST_PID / LUCIDOS_FRONTEND_PID — the CC subprocess inherits
#     these from the engine (api/actor.rs host_protection_env_vars). Even when
#     a test script invokes ports.sh for a different workspace (e.g. e2e-test),
#     they still identify the dev-workspace engine that is running the test.
#  2. A scan of every workspace's engine under the user's home and its e2e
#     root, covering the case where the env vars are absent (manual dev script
#     invocation, no CC chain) but sibling workspaces are alive. A pid counts only while its
#     engine.lock is held, so a stale pidfile naming a recycled PID can't
#     protect an unrelated process.
#  3. ANCESTOR — this process and everything it descends from. A process cannot
#     unset its own parentage, so no caller can defeat this one.
#  4. pid <= 1 — init/the kernel are never ours to signal (mirrors the same
#     guard in webkit_reaper.sh reap_once).
#
# Arms 3 and 4 exist because arms 1 and 2 BOTH failed open on 2026-07-28 and
# this library's own test suite killed the machine's live dev engine, twice:
# the tests unset the env vars and point HOME at a mktemp dir, so the engine
# was invisible to both arms, the cmdline matched *lucidos-engine*, and the
# reclaim path SIGUSR1'd it. Arm 2 is scanned under the password-database home
# as well as $HOME for the same reason — reassigning HOME must not disarm
# protection. See ADR 0025.
is_protected_host_pid() {
    local pid="$1"
    [ -z "$pid" ] && return 1
    case "$pid" in *[!0-9]*) return 1 ;; esac

    [ "$pid" -le 1 ] && return 0

    if [ -n "${LUCIDOS_HOST_PID:-}" ] && [ "$pid" = "$LUCIDOS_HOST_PID" ]; then
        return 0
    fi
    if [ -n "${LUCIDOS_FRONTEND_PID:-}" ] && [ "$pid" = "$LUCIDOS_FRONTEND_PID" ]; then
        return 0
    fi

    _ensure_ancestor_pid_set
    case "$_LUCIDOS_ANCESTOR_PIDS" in
        *" $pid "*) return 0 ;;
    esac

    _ensure_passwd_home
    local dirs=() root pidfile ws
    for root in "${HOME:-}" "$_LUCIDOS_PASSWD_HOME"; do
        if [ -n "$root" ]; then
            dirs+=("$root/workspaces" "$root/$E2E_ROOT_HOME_SUBPATH/workspaces")
        fi
    done
    if [ -n "${LUCIDOS_E2E_ROOT:-}" ]; then
        dirs+=("$LUCIDOS_E2E_ROOT/workspaces")
    fi
    for root in "${dirs[@]}"; do
        for pidfile in "$root"/*/.lucidos/engine.pid; do
            [ "$(cat "$pidfile" 2>/dev/null)" = "$pid" ] || continue
            ws="${pidfile%/.lucidos/engine.pid}"
            [ "$(engine_live_pid "$ws")" = "$pid" ] && return 0
        done
    done
    return 1
}

# Filter `kill <pids>` so protected host PIDs are skipped with a stderr
# notice. Reads one pid per line on stdin (matches what `lsof -ti` emits).
# `signal` defaults to TERM; pass `-9` for SIGKILL.
#
# The `|| [ -n "$pid" ]` keeps the loop running for a final line with no
# trailing newline — `read` returns non-zero in that case but still populates
# `$pid`. Safe with whitespace via `IFS=`.
kill_unprotected_pids() {
    local signal="${1:-}"
    local pid
    while IFS= read -r pid || [ -n "$pid" ]; do
        [ -z "$pid" ] && continue
        if is_protected_host_pid "$pid"; then
            echo "ports.sh: refusing to signal protected host pid $pid" >&2
            continue
        fi
        if [ -n "$signal" ]; then
            kill "$signal" "$pid" 2>/dev/null || true
        else
            kill "$pid" 2>/dev/null || true
        fi
    done
}

# Look up the offset for a workspace in the global registry.
# Returns the offset via stdout, or empty string if not found.
registry_lookup() {
    local workspace="$1"
    if [ -f "$LUCIDOS_PORT_REGISTRY" ]; then
        awk -F'\t' -v ws="$workspace" '$1 == ws { print $2; exit }' "$LUCIDOS_PORT_REGISTRY"
    fi
}

# Get the next available offset (max existing + 1, or 0 if registry is empty).
registry_next_offset() {
    if [ ! -f "$LUCIDOS_PORT_REGISTRY" ] || [ ! -s "$LUCIDOS_PORT_REGISTRY" ]; then
        echo 0
        return
    fi
    awk -F'\t' 'BEGIN{m=-1} {if($2+0>m) m=$2+0} END{print m+1}' "$LUCIDOS_PORT_REGISTRY"
}

# The workspace other than `workspace` that owns `offset` in the registry, or
# nothing.
registry_offset_owner() { # <offset> <workspace>
    if [ -f "$LUCIDOS_PORT_REGISTRY" ]; then
        awk -F'\t' -v o="$1" -v ws="$2" '$2 == o && $1 != ws { print $1; exit }' "$LUCIDOS_PORT_REGISTRY"
    fi
}

# Register a workspace's offset, replacing any prior entry. The caller holds the
# registry lock (allocate_ports), and the rename means a reader never sees a
# partial file.
registry_save() {
    local workspace="$1"
    local offset="$2"
    local tmp
    tmp=$(mktemp "$LUCIDOS_PORT_REGISTRY.tmp.XXXXXX") || return 1
    if [ -f "$LUCIDOS_PORT_REGISTRY" ]; then
        awk -F'\t' -v ws="$workspace" '$1 != ws' "$LUCIDOS_PORT_REGISTRY" > "$tmp"
    fi
    printf '%s\t%s\n' "$workspace" "$offset" >> "$tmp"
    mv "$tmp" "$LUCIDOS_PORT_REGISTRY"
}

# Validate an engine-port pin: a positive integer at or above
# LUCIDOS_ENGINE_PORT_BASE, so the offset stays non-negative. Run it after
# translate_legacy_engine_port. Echoes an error to stderr and returns non-zero on
# rejection. $2 names the source in the message (e.g. "LUCIDOS_VITE_PORT").
_validate_vite_port() {
    local value="$1"
    local source="$2"
    if ! [[ "$value" =~ ^[0-9]+$ ]]; then
        echo "ERROR: $source=$value is not a positive integer" >&2
        return 1
    fi
    if [ "$value" -lt "$LUCIDOS_ENGINE_PORT_BASE" ]; then
        echo "ERROR: $source=$value is below the $LUCIDOS_ENGINE_PORT_BASE minimum for a dev engine port" >&2
        return 1
    fi
    return 0
}

# The same offset on the new base when `port` lies in the legacy range, else
# `port` unchanged. The legacy range is one walk wide from the old base.
translate_legacy_engine_port() { # <port>
    local port="$1"
    if [[ "$port" =~ ^[0-9]+$ ]] \
        && [ "$port" -ge "$LUCIDOS_LEGACY_ENGINE_PORT_BASE" ] \
        && [ "$port" -lt $(( LUCIDOS_LEGACY_ENGINE_PORT_BASE + LUCIDOS_PORT_WALK_LIMIT )) ]; then
        echo $(( port - LUCIDOS_LEGACY_ENGINE_PORT_BASE + LUCIDOS_ENGINE_PORT_BASE ))
    else
        echo "$port"
    fi
}

# Rewrite `vite = <old>` under [ports] in the workspace's lucidos.toml to `new`,
# keeping every other line. The engine commits the file when it writes it, so a
# tracked file gets a commit too. A failed commit only warns: the pin itself is
# what the launch needs.
_rewrite_lucidos_toml_vite_port() { # <workspace> <old> <new>
    local workspace="$1" old="$2" new="$3"
    local toml="$workspace/lucidos.toml" tmp
    tmp=$(mktemp "$toml.tmp.XXXXXX") || return 1
    awk -v new="$new" '
        /^[[:space:]]*\[/ {
            in_section = ($0 ~ /^[[:space:]]*\[ports\][[:space:]]*$/) ? 1 : 0
        }
        in_section && !done && /^[[:space:]]*vite[[:space:]]*=/ {
            print "vite = " new
            done = 1
            next
        }
        { print }
    ' "$toml" > "$tmp" || { rm -f "$tmp"; return 1; }
    mv "$tmp" "$toml" || { rm -f "$tmp"; return 1; }
    echo "[ports] moved the engine port pin in $toml from $old to $new: dev ports left the range other tools use (ADR 0821)" >&2
    if git -C "$workspace" ls-files --error-unmatch lucidos.toml >/dev/null 2>&1; then
        git -C "$workspace" commit -q --no-verify --only \
            -m "chore: move the pinned engine port off the old dev range" -- lucidos.toml >/dev/null 2>&1 \
            || echo "WARNING: could not commit $toml; the new pin is in place but uncommitted" >&2
    fi
}

# Read the workspace's lucidos.toml for [ports] vite = N.
# Tiny hand-rolled awk parser — covers `vite = N`, `vite=N`, `vite = "N"` under
# `[ports]`. Does NOT handle inline tables (`ports = { vite = N }`); add a
# real toml dep if that becomes a need.
read_lucidos_toml_vite_port() {
    local workspace="$1"
    local toml="$workspace/lucidos.toml"
    [ -f "$toml" ] || return 0
    awk '
        /^[[:space:]]*\[/ {
            in_section = ($0 ~ /^[[:space:]]*\[ports\][[:space:]]*$/) ? 1 : 0
            next
        }
        in_section && /^[[:space:]]*vite[[:space:]]*=/ {
            sub(/^[^=]*=[[:space:]]*/, "")
            sub(/[[:space:]#].*$/, "")
            gsub(/"/, "")
            print
            exit
        }
    ' "$toml"
}

# How long a health probe may take. A launch must never block on a socket that
# never answers, and the gateway's own restart-intent call uses the same budget.
LUCIDOS_HEALTH_PROBE_TIMEOUT_S=2

# ── probe_engine_on_port ────────────────────────────────────────────────
# Ask the engine on `port` which workspace it serves.
#
# `GET /api/v1/health` carries `workspace_path`, so the engine itself settles
# which workspace a port serves. A pid names a process, not a workspace. See
# docs/plans/2026-09-18-a-launch-never-tears-down-a-healthy-engine.md.
#
# Answers out of band, the way `_port_is_ours_or_free` passes `OCCUPIER_PID`,
# because a caller needs both halves and a command substitution would lose the
# second. Returns 0 only when a workspace was reported.
#
#   ENGINE_PROBE_WORKSPACE  what it said, or empty
#   ENGINE_PROBE_REFUSED    set when NOTHING accepted the connection
#   ENGINE_PROBE_BOOTING    set on the explicit boot response
#   ENGINE_PROBE_TIMED_OUT  set if either request timed out
#
# HTTPS first, then HTTP, like engine_health_scheme in workspace.sh.
ENGINE_PROBE_WORKSPACE=""
ENGINE_PROBE_REFUSED=""
ENGINE_PROBE_BOOTING=""
ENGINE_PROBE_TIMED_OUT=""
probe_engine_on_port() { # <port>
    ENGINE_PROBE_WORKSPACE=""
    ENGINE_PROBE_REFUSED=""
    ENGINE_PROBE_BOOTING=""
    ENGINE_PROBE_TIMED_OUT=""
    local port="$1" response="" scheme rc status refused=0
    [ -n "$port" ] || return 1
    for scheme in https http; do
        rc=0
        response="$(curl -ski --max-time "$LUCIDOS_HEALTH_PROBE_TIMEOUT_S" -w '\n%{http_code}' \
            "$scheme://localhost:$port/api/v1/health" 2>/dev/null)" || rc=$?
        case "$rc" in
            0) ;;
            7) refused=$((refused + 1)); continue ;;
            28) ENGINE_PROBE_TIMED_OUT=1; continue ;;
            *) continue ;;
        esac
        status="${response##*$'\n'}"
        case "$status" in
            503)
                if printf '%s\n' "$response" | awk '
                    { sub(/\r$/, "") }
                    /^$/ { exit }
                    tolower($0) ~ /^x-lucidos-engine-booting:[ \t]*1[ \t]*$/ { booting=1 }
                    END { exit !booting }
                '; then
                    ENGINE_PROBE_BOOTING=1
                    return 1
                fi
                ;;
            2[0-9][0-9])
                ENGINE_PROBE_WORKSPACE="$(printf '%s\n' "$response" \
                    | awk 'body { print } /^\r?$/ { body=1 }' \
                    | sed -n 's/.*"workspace_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
                [ -z "$ENGINE_PROBE_WORKSPACE" ] || return 0
                ;;
        esac
    done
    if [ "$refused" -eq 2 ]; then
        ENGINE_PROBE_REFUSED=1
    fi
    return 1
}

# The physical path of `dir`, or the literal when it cannot be resolved. Two
# spellings of one directory have to compare equal.
_path_identity() { # <dir>
    local dir="${1%/}"
    ( cd "$dir" 2>/dev/null && pwd -P ) || printf '%s' "$dir"
}

# True when the engine answering on `port` serves `workspace`.
engine_on_port_serves_workspace() { # <port> <workspace-dir>
    probe_engine_on_port "$1" || return 1
    [ "$(_path_identity "$ENGINE_PROBE_WORKSPACE")" = "$(_path_identity "$2")" ]
}

# Try to reclaim a port held by a stale lucidos-engine — one of ours that
# crashed or got orphaned, with no live pidfile claiming it. Returns 0 if
# the port is free afterwards, 1 if we shouldn't touch the occupier (an
# ancestor of this process, any live other-workspace pidfile claim, an
# env-var-protected host, or a non-lucidos cmdline). Without this,
# allocate_ports would silently walk past the registered offset every time a
# workspace's own crashed engine was still listening, drifting the offset on
# each restart and persisting the drift back to the registry.
_try_reclaim_stale_lucidos_on_port() {
    local port="$1"
    local pid="$2"
    [ -z "$pid" ] && return 1

    # Protected → leave it alone. This gate is what stands between a port
    # collision and a dead engine: everything below it signals, and the
    # cmdline check below is NOT a safety net (a live host engine matches
    # *lucidos-engine* by definition — that is how the 2026-07-28 incident
    # got all the way to `kill -USR1`). See is_protected_host_pid, ADR 0025.
    is_protected_host_pid "$pid" && return 1

    # Only reclaim if the cmdline is a lucidos-engine. Vite children exit
    # when their parent engine dies, so the engine is the only practical
    # orphan worth chasing.
    local cmd
    cmd=$(ps -p "$pid" -o command= 2>/dev/null | tr -d '\n')
    case "$cmd" in
        *lucidos-engine*) ;;
        *) return 1 ;;
    esac

    # An engine that answers health is not stale, which is this function's whole
    # premise. The arms above scan only `~/workspaces`, so an engine elsewhere
    # looks orphaned to them, and the SIGUSR1 below would tear down every
    # in-flight thread in its workspace. See
    # docs/plans/2026-09-18-a-launch-never-tears-down-a-healthy-engine.md.
    #
    # Ask twice unless the port explicitly refuses both connections or reports
    # its boot state. A timeout leaves the holder's state uncertain.
    local _probe timed_out=""
    for _probe in 1 2; do
        if probe_engine_on_port "$port"; then
            echo "[ports] port $port answers /api/v1/health, leaving the live engine (pid $pid) alone" >&2
            return 1
        fi
        if [ -n "$ENGINE_PROBE_BOOTING" ]; then
            echo "[ports] port $port is held by a booting engine (pid $pid)" >&2
            return 1
        fi
        if [ -n "$ENGINE_PROBE_REFUSED" ]; then break; fi
        if [ -n "$ENGINE_PROBE_TIMED_OUT" ]; then timed_out=1; fi
    done

    if [ -n "$timed_out" ] && [ -z "$ENGINE_PROBE_REFUSED" ]; then
        return 1
    fi

    echo "[ports] reclaiming stale lucidos-engine (pid $pid) on port $port" >&2

    # Engine ignores SIGTERM to survive accidental xargs-kill from CC test
    # scripts (see main.rs shutdown_signal); SIGUSR1 is the legitimate stop
    # signal. Mirrors workspace.sh kill_stale_processes.
    kill -USR1 "$pid" 2>/dev/null || true

    # 6 × 0.5s = 3s budget for SIGUSR1 shutdown before escalating.
    for _ in 1 2 3 4 5 6; do
        sleep 0.5
        port_is_free "$port" && return 0
    done

    # Escalate — SIGTERM then SIGKILL. The engine ignores TERM, but the
    # try is cheap and protects against the case where the occupier is a
    # non-engine binary that happens to match `*lucidos-engine*` in argv.
    # `kill -0` first so a recycled PID (engine exited mid-poll, kernel
    # handed the number to an unrelated process) doesn't get signalled.
    kill -0 "$pid" 2>/dev/null || return 1
    kill "$pid" 2>/dev/null || true
    sleep 0.5
    port_is_free "$port" && return 0

    kill -0 "$pid" 2>/dev/null || return 1
    kill -9 "$pid" 2>/dev/null || true
    sleep 0.5
    port_is_free "$port" && return 0

    return 1
}

# True if a port is either free or held by this workspace's own engine. Lets a
# restart reuse the same ports: port_is_free alone would walk past our own
# engine. On false, OCCUPIER_PID is set to the foreign process's pid (empty if
# we couldn't read one).
_port_is_ours_or_free() {
    local port="$1"
    local workspace="$2"
    OCCUPIER_PID=""

    port_is_free "$port" && return 0

    OCCUPIER_PID=$(lsof -ti :"$port" -sTCP:LISTEN 2>/dev/null | head -1)
    if [ -n "$OCCUPIER_PID" ]; then
        # _allocate_ports_locked looks the engine up once for the whole walk.
        local own="${ALLOCATING_ENGINE_PID-$(engine_live_pid "$workspace")}"
        if [ -n "$own" ] && [ "$OCCUPIER_PID" = "$own" ]; then
            OCCUPIER_PID=""
            return 0
        fi

        # Ask the port which workspace it serves. That also covers the pid
        # `head -1` did not pick, and an engine still draining.
        if engine_on_port_serves_workspace "$port" "$workspace"; then
            OCCUPIER_PID=""
            return 0
        fi

        # Foreign-looking pid that's actually our own crashed engine? Try
        # to reclaim before declaring the port lost and walking forward.
        if _try_reclaim_stale_lucidos_on_port "$port" "$OCCUPIER_PID"; then
            OCCUPIER_PID=""
            return 0
        fi
    fi

    return 1
}

# The previous launch's engine port, from the workspace ports file.
_recorded_engine_port() { # <ports-file>
    [ -f "$1" ] || return 0
    sed -n 's/^VITE_PORT=//p' "$1" | head -n 1
}

# The legacy port to keep for this launch, or nothing. A workspace whose own
# engine still answers on its legacy port stays there: the gateway refuses to
# move a running engine, and the ports file must name where it really listens.
_legacy_port_still_served() { # <workspace> <new-port>
    local workspace="$1" new="$2" old
    old="$(_recorded_engine_port "$workspace/.lucidos/ports")"
    [ -n "$old" ] && [ "$old" != "$new" ] || return 0
    [ "$(translate_legacy_engine_port "$old")" != "$old" ] || return 0
    engine_on_port_serves_workspace "$old" "$workspace" || return 0
    echo "$old"
}

# Say once that this workspace's engine left the legacy range. The ports file
# still holds the previous launch's port, so the line prints on the first launch
# after the move and never again.
_announce_moved_engine_port() { # <ports-file> <new-port>
    local old
    old="$(_recorded_engine_port "$1")"
    [ -n "$old" ] && [ "$old" != "$2" ] || return 0
    [ "$(translate_legacy_engine_port "$old")" != "$old" ] || return 0
    echo "[ports] this workspace's engine moved from port $old to $2: dev ports left the range other tools use (ADR 0821). A bookmark of localhost:$old stops working." >&2
}

# Set + export the three port vars from an offset, persist the offset, write
# the workspace ports file, and emit the chosen-ports log line. Shared by the
# normal and ENGINE_ONLY paths so a new export/log field can't drift between
# them.
_finalize_ports() {
    local workspace="$1"
    local offset="$2"
    local source="$3"
    local mode="$4"  # "" or "engine-only" — affects the log suffix only

    VITE_PORT=$(( LUCIDOS_ENGINE_PORT_BASE + offset ))
    local kept
    kept="$(_legacy_port_still_served "$workspace" "$VITE_PORT")"
    if [ -n "$kept" ]; then
        echo "[ports] this workspace's engine still runs on its old port $kept. It moves to $VITE_PORT once stopped: ./scripts/stop.sh -w $workspace, then launch again (ADR 0821)." >&2
        VITE_PORT="$kept"
    fi
    API_PORT="$VITE_PORT"
    PG_PORT="$LUCIDOS_SHARED_PG_PORT"
    export API_PORT VITE_PORT PG_PORT

    # A silent miss here would leave future restarts unable to find the offset,
    # so they would walk to a fresh one.
    registry_save "$workspace" "$offset" || return 1

    _announce_moved_engine_port "$workspace/.lucidos/ports" "$VITE_PORT"
    ports_file_set "$workspace/.lucidos/ports" \
        "API_PORT=$VITE_PORT" "VITE_PORT=$VITE_PORT"

    local suffix=""
    [ -n "$mode" ] && suffix=" ($mode)"
    echo "[ports] $source$suffix → engine=$VITE_PORT PG=$PG_PORT (offset $offset, workspace=$workspace)" >&2
}

# Allocate ports for a workspace. See the file header for precedence + collision
# rules. Sets and exports API_PORT, VITE_PORT, PG_PORT.
#
# Holds the registry lock across the whole allocation, health probes and
# reclaims included. A concurrent launch waits, and flock_open says so.
allocate_ports() {
    local workspace="$1" rc=0
    mkdir -p "$workspace/.lucidos"
    ensure_storage_root "$(dirname "$LUCIDOS_PORT_REGISTRY")"
    flock_open "$PORT_REGISTRY_LOCK_FD" "$LUCIDOS_PORT_REGISTRY.lock" ex || return 1
    _allocate_ports_locked "$workspace" || rc=$?
    flock_close "$PORT_REGISTRY_LOCK_FD"
    return "$rc"
}

_allocate_ports_locked() {
    local workspace="$1"
    local ALLOCATING_ENGINE_PID
    ALLOCATING_ENGINE_PID="$(engine_live_pid "$workspace")"

    # Resolve the *desired* starting offset.
    local source="default"
    local offset=""
    if [ -n "${LUCIDOS_VITE_PORT:-}" ]; then
        local env_vite
        env_vite="$(translate_legacy_engine_port "$LUCIDOS_VITE_PORT")"
        if [ "$env_vite" != "$LUCIDOS_VITE_PORT" ]; then
            echo "[ports] LUCIDOS_VITE_PORT=$LUCIDOS_VITE_PORT is in the old dev range, using $env_vite for this launch. Set LUCIDOS_VITE_PORT=$env_vite to silence this (ADR 0821)." >&2
        fi
        if ! _validate_vite_port "$env_vite" "LUCIDOS_VITE_PORT"; then
            return 1
        fi
        offset=$(( env_vite - LUCIDOS_ENGINE_PORT_BASE ))
        source="env LUCIDOS_VITE_PORT=$env_vite"
    else
        local toml_vite moved_vite
        toml_vite=$(read_lucidos_toml_vite_port "$workspace")
        if [ -n "$toml_vite" ]; then
            moved_vite="$(translate_legacy_engine_port "$toml_vite")"
            if [ "$moved_vite" != "$toml_vite" ]; then
                _rewrite_lucidos_toml_vite_port "$workspace" "$toml_vite" "$moved_vite" || return 1
                toml_vite="$moved_vite"
            fi
            if ! _validate_vite_port "$toml_vite" "lucidos.toml [ports] vite"; then
                return 1
            fi
            offset=$(( toml_vite - LUCIDOS_ENGINE_PORT_BASE ))
            source="lucidos.toml vite=$toml_vite"
        fi
    fi

    if [ -z "$offset" ]; then
        offset=$(registry_lookup "$workspace")
        if [ -n "$offset" ]; then
            source="registry"
        else
            offset=$(registry_next_offset)
            source="auto offset=$offset"
        fi
    fi

    # ENGINE_ONLY mode: only the engine restarts, so the offset is already
    # settled and we MUST NOT walk. start_engine checks the port is free.
    if [ -n "${ENGINE_ONLY:-}" ]; then
        _finalize_ports "$workspace" "$offset" "$source" "engine-only"
        return $?
    fi

    # A PINNED offset (lucidos.toml [ports] vite, or LUCIDOS_VITE_PORT) is
    # authoritative: NEVER walk off it, which would defeat the pin. Reclaim our
    # OWN stale engine squatting the pinned port, then REQUIRE it to be free. A
    # genuine foreign squatter is an error, never a drift to another port.
    case "$source" in
        "lucidos.toml"* | "env LUCIDOS_VITE_PORT"*)
            local pinned=$(( LUCIDOS_ENGINE_PORT_BASE + offset ))
            # Reclaims a stale lucidos-engine (the SIGUSR1 path, since engines
            # ignore SIGTERM). Its stderr is deliberately not swallowed: a launch
            # that stops an engine has to say so.
            if ! _port_is_ours_or_free "$pinned" "$workspace"; then
                # Kill an unprotected (orphaned) listener left by a dead session.
                # Protected host pids are skipped. `|| true`: lsof exits non-zero
                # when the port is free, which `set -e` would turn into an abort.
                local occ
                occ=$(lsof -ti :"$pinned" -sTCP:LISTEN 2>/dev/null || true)
                [ -n "$occ" ] && printf '%s\n' "$occ" | kill_unprotected_pids || true
                sleep 0.3
                if ! _port_is_ours_or_free "$pinned" "$workspace"; then
                    echo "ERROR: pinned port for workspace '$workspace' is occupied: $pinned (pid ${OCCUPIER_PID:-unknown})" >&2
                    echo "       Source: $source." >&2
                    echo "       Free that port or change the pin in $workspace/lucidos.toml. A pinned port is never walked off." >&2
                    return 1
                fi
            fi
            _finalize_ports "$workspace" "$offset" "$source" ""
            return $?
            ;;
    esac

    # Walk forward until the engine port is free (or held by us). PG is
    # deliberately NOT gated here: a PG-port collision must never drift the
    # engine port. setup_postgres (`_ensure_shared_postgres_container`) resolves
    # its own free port and reuses a running container's published one.
    local steps=0
    while :; do
        local candidate=$(( LUCIDOS_ENGINE_PORT_BASE + offset ))

        # An offset another workspace registered is theirs even while their
        # engine is down, so the walk never lands on it. The starting offset is
        # exempt: a duplicate an older allocator left must not move a workspace
        # off its own registered offset while its engine runs there.
        local owner=""
        [ "$steps" -eq 0 ] || owner="$(registry_offset_owner "$offset" "$workspace")"
        if [ -n "$owner" ]; then
            echo "[ports] offset $offset belongs to $owner, walking forward" >&2
        elif _port_is_ours_or_free "$candidate" "$workspace"; then
            break
        else
            # OCCUPIER_PID is set by the failing check so we can name the
            # squatter without re-running lsof.
            local cmd=""
            [ -n "$OCCUPIER_PID" ] && cmd=$(ps -p "$OCCUPIER_PID" -o comm= 2>/dev/null | tr -d ' ' || true)
            echo "[ports] port $candidate occupied${cmd:+ by $cmd (pid $OCCUPIER_PID)}, walking forward" >&2
        fi

        offset=$(( offset + 1 ))
        steps=$(( steps + 1 ))
        if [ "$steps" -gt "$LUCIDOS_PORT_WALK_LIMIT" ]; then
            echo "ERROR: walked $LUCIDOS_PORT_WALK_LIMIT offsets without finding a free engine port" >&2
            return 1
        fi
    done

    _finalize_ports "$workspace" "$offset" "$source" ""
}
