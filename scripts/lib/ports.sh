#!/bin/bash
# Port allocation for multi-workspace support.
#
# Each workspace gets a stable port offset stored in ~/.lucidos/port-registry,
# applied uniformly to API (3000+offset), Vite (5173+offset), and PG (5432+offset).
#
# Precedence for the desired vite port:
#   1. LUCIDOS_VITE_PORT env var  (highest — for one-off "I need a specific port")
#   2. <workspace>/lucidos.toml   [ports] vite = N  (per-workspace pin)
#   3. registry entry             (stable offset assigned on first run)
#   4. next-free offset           (max(registry) + 1, or 0 if empty)
#
# On collision (the desired vite or api port is occupied by something that
# isn't ours), walk the offset forward by 1 until both are free. The walked
# offset is persisted in the registry.
#
# Exports: API_PORT, VITE_PORT, PG_PORT.

LUCIDOS_PORT_REGISTRY="$HOME/.lucidos/port-registry"
LUCIDOS_PORT_WALK_LIMIT=1000

# ── ports_file_set ──────────────────────────────────────────────────────
# Set KEY=VALUE lines in a workspace's ports file, preserving every other line.
#
# The shell twin of merge_ports_file in crates/lucidos-gateway/src/stack.rs, and
# it exists for the same reason: this file has several owners. allocate_ports
# writes the two port keys, detect_tls writes PROTO, swap_ports writes the
# Postgres keys, and the gateway republishes API_PORT and PROTO on every spawn
# and re-adoption. scripts/status.sh sources the result.
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
# never signal — the engine that spawned us, the matching frontend, or any
# other workspace's recorded engine/frontend.
#
# FOUR arms. The first two read caller-owned state and a caller can switch
# them off; the last two cannot, which is the whole point.
#
#  1. LUCIDOS_HOST_PID / LUCIDOS_FRONTEND_PID — the CC subprocess inherits
#     these from the engine (api/actor.rs host_protection_env_vars). Even when
#     a test script invokes ports.sh for a different workspace (e.g. e2e-test),
#     they still identify the dev-workspace engine that is running the test.
#  2. A pidfile scan across every workspace under the user's home, covering the
#     case where the env vars are absent (manual dev script invocation, no CC
#     chain) but sibling workspaces are alive. `kill -0` gates each match so a
#     stale pidfile naming a recycled PID can't protect an unrelated process.
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
    local root pidfile other_pid
    for root in "${HOME:-}" "$_LUCIDOS_PASSWD_HOME"; do
        [ -n "$root" ] || continue
        for pidfile in "$root"/workspaces/*/.lucidos/engine.pid "$root"/workspaces/*/.lucidos/frontend.pid; do
            [ -f "$pidfile" ] || continue
            other_pid="$(cat "$pidfile" 2>/dev/null)"
            [ -z "$other_pid" ] && continue
            if [ "$pid" = "$other_pid" ] && kill -0 "$other_pid" 2>/dev/null; then
                return 0
            fi
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

# Acquire the registry lock via atomic mkdir (portable; macOS has no
# flock). The PID inside <lockdir>/owner lets a later caller reclaim a
# lock held by a dead process.
_acquire_registry_lock() {
    local lockdir="$LUCIDOS_PORT_REGISTRY.lockd"
    local owner_file="$lockdir/owner"
    local tries=0
    while ! mkdir "$lockdir" 2>/dev/null; do
        if [ -f "$owner_file" ]; then
            local owner_pid
            owner_pid=$(cat "$owner_file" 2>/dev/null)
            if [ -n "$owner_pid" ] && ! kill -0 "$owner_pid" 2>/dev/null; then
                rm -f "$owner_file"
                rmdir "$lockdir" 2>/dev/null || true
                continue
            fi
        fi
        tries=$((tries+1))
        # 500 × 20ms = 10s budget. Save is sub-ms; this only matters
        # under heavy boot-time contention.
        if [ "$tries" -gt 500 ]; then
            echo "ports.sh: gave up waiting for registry lock at $lockdir" >&2
            return 1
        fi
        sleep 0.02
    done
    echo "$$" > "$owner_file"
    return 0
}

_release_registry_lock() {
    local lockdir="$LUCIDOS_PORT_REGISTRY.lockd"
    rm -f "$lockdir/owner"
    rmdir "$lockdir" 2>/dev/null || true
}

# Register a workspace's offset, replacing any prior entry. Atomic against
# concurrent writers; a unique tempfile under the lock means even a partial
# overlap can't clobber.
registry_save() {
    local workspace="$1"
    local offset="$2"
    mkdir -p "$(dirname "$LUCIDOS_PORT_REGISTRY")"

    _acquire_registry_lock || return 1
    # Critical section in a subshell so its EXIT trap releases the lock
    # without clobbering any EXIT trap the caller installed.
    (
        trap '_release_registry_lock' EXIT
        local tmp
        tmp=$(mktemp "$LUCIDOS_PORT_REGISTRY.tmp.XXXXXX") || exit 1
        if [ -f "$LUCIDOS_PORT_REGISTRY" ]; then
            awk -F'\t' -v ws="$workspace" '$1 != ws' "$LUCIDOS_PORT_REGISTRY" > "$tmp"
        else
            : > "$tmp"
        fi
        printf '%s\t%s\n' "$workspace" "$offset" >> "$tmp"
        mv "$tmp" "$LUCIDOS_PORT_REGISTRY"
    )
}

# Validate a vite-port override: must be a positive integer ≥ 5173 so the
# derived API and PG offsets stay non-negative. Echoes an error to stderr and
# returns non-zero on rejection. $2 is the source name used in the error
# message (e.g. "LUCIDOS_VITE_PORT" or "lucidos.toml [ports] vite").
_validate_vite_port() {
    local value="$1"
    local source="$2"
    if ! [[ "$value" =~ ^[0-9]+$ ]]; then
        echo "ERROR: $source=$value is not a positive integer" >&2
        return 1
    fi
    if [ "$value" -lt 5173 ]; then
        echo "ERROR: $source=$value is below the 5173 minimum (would yield negative API/PG ports)" >&2
        return 1
    fi
    return 0
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

# True if a port is either free or held by a process we own (engine pidfile or
# frontend pidfile of this workspace). Lets a restart reuse the same ports —
# port_is_free alone would walk past our own engine. On false, OCCUPIER_PID is
# set to the foreign process's pid (empty if we couldn't read one).
_port_is_ours_or_free() {
    local port="$1"
    local workspace="$2"
    OCCUPIER_PID=""

    port_is_free "$port" && return 0

    OCCUPIER_PID=$(lsof -ti :"$port" -sTCP:LISTEN 2>/dev/null | head -1)
    if [ -n "$OCCUPIER_PID" ]; then
        local engine_pid_file="$workspace/.lucidos/engine.pid"
        local frontend_pid_file="$workspace/.lucidos/frontend.pid"
        if [ -f "$engine_pid_file" ] && [ "$OCCUPIER_PID" = "$(cat "$engine_pid_file" 2>/dev/null)" ]; then
            OCCUPIER_PID=""
            return 0
        fi
        if [ -f "$frontend_pid_file" ] && [ "$OCCUPIER_PID" = "$(cat "$frontend_pid_file" 2>/dev/null)" ]; then
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

# True if a PG port is either free or held by this workspace's own Docker
# container (`lucidos-pg-$PG_NAME`). PG_NAME is a global set by
# resolve_workspace before allocate_ports runs. On false, OCCUPIER_PID is
# set to the listener pid (the Docker proxy) for the walk-forward log line.
_pg_port_is_ours_or_free() {
    local port="$1"
    OCCUPIER_PID=""

    port_is_free "$port" && return 0

    # Check if the port is held by our own PG container.
    if [ -n "${PG_NAME:-}" ]; then
        local container
        container=$(docker ps --filter "publish=$port" --format "{{.Names}}" 2>/dev/null | head -1)
        if [ -n "$container" ] && [ "$container" = "lucidos-pg-$PG_NAME" ]; then
            return 0
        fi
    fi

    OCCUPIER_PID=$(lsof -ti :"$port" -sTCP:LISTEN 2>/dev/null | head -1)
    return 1
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

    API_PORT=$(( 3000 + offset ))
    VITE_PORT=$(( 5173 + offset ))
    PG_PORT=$(( 5432 + offset ))
    export API_PORT VITE_PORT PG_PORT

    # Propagate registry_save failures (lock-acquire timeout, mktemp). A
    # silent miss here means future restarts can't find the offset and
    # walk to a fresh one — the exact port-drift the lock prevents.
    registry_save "$workspace" "$offset" || return 1

    # Both keys record the user-facing engine port (== post-swap VITE_PORT).
    # The raw API_PORT (3000-range) is Vite's internal port after swap and
    # must not leak to consumers.
    ports_file_set "$workspace/.lucidos/ports" \
        "API_PORT=$VITE_PORT" "VITE_PORT=$VITE_PORT"

    local suffix=""
    [ -n "$mode" ] && suffix=" ($mode)"
    echo "[ports] $source$suffix → API=$API_PORT VITE=$VITE_PORT PG=$PG_PORT (offset $offset, workspace=$workspace)" >&2
}

# Allocate ports for a workspace. See the file header for precedence + collision
# rules. Sets and exports API_PORT, VITE_PORT, PG_PORT.
allocate_ports() {
    local workspace="$1"
    mkdir -p "$workspace/.lucidos"

    # Resolve the *desired* starting offset.
    local source="default"
    local offset=""
    if [ -n "${LUCIDOS_VITE_PORT:-}" ]; then
        if ! _validate_vite_port "$LUCIDOS_VITE_PORT" "LUCIDOS_VITE_PORT"; then
            return 1
        fi
        offset=$(( LUCIDOS_VITE_PORT - 5173 ))
        source="env LUCIDOS_VITE_PORT=$LUCIDOS_VITE_PORT"
    else
        local toml_vite
        toml_vite=$(read_lucidos_toml_vite_port "$workspace")
        if [ -n "$toml_vite" ]; then
            if ! _validate_vite_port "$toml_vite" "lucidos.toml [ports] vite"; then
                return 1
            fi
            offset=$(( toml_vite - 5173 ))
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

    # ENGINE_ONLY mode: the engine is restarting while Vite + PG are still
    # running. The offset is already pinned by the running Vite, so we MUST NOT
    # walk — pick the offset, set the vars, return. Verification that the
    # engine port is really free happens in start_engine.
    if [ -n "${ENGINE_ONLY:-}" ]; then
        _finalize_ports "$workspace" "$offset" "$source" "engine-only"
        return $?
    fi

    # A PINNED offset (lucidos.toml [ports] vite, or LUCIDOS_VITE_PORT) is
    # authoritative: NEVER walk off it — that defeats the whole point of pinning.
    # Reclaim our OWN stale processes squatting the pinned ports (a crashed engine
    # on the user-facing port, an orphaned Vite the in-app engine-only restart
    # left on the internal port), then REQUIRE the ports to be free. If a genuine
    # foreign squatter remains, error out — do not drift to a different port.
    case "$source" in
        "lucidos.toml"* | "env LUCIDOS_VITE_PORT"*)
            local pvite=$(( 5173 + offset ))
            local papi=$(( 3000 + offset ))
            # Reclaim a stale lucidos-engine on the user-facing port (SIGUSR1
            # path — engines ignore SIGTERM). No-op when free or ours.
            _port_is_ours_or_free "$pvite" "$workspace" >/dev/null 2>&1 || true
            # Kill unprotected (orphaned) listeners on both ports — our own
            # leftover engine/Vite from a dead session. Live host pids recorded in
            # any workspace's engine/frontend pidfile are skipped.
            local cp occ
            for cp in "$pvite" "$papi"; do
                # `|| true`: lsof exits non-zero when the port is free, which under
                # `set -e` would abort the whole script on a bare assignment.
                occ=$(lsof -ti :"$cp" -sTCP:LISTEN 2>/dev/null || true)
                [ -n "$occ" ] && printf '%s\n' "$occ" | kill_unprotected_pids || true
            done
            sleep 0.3
            # Both must now be free (or our own reuse). A remaining squatter is a
            # hard error — refuse to walk forward off the pin.
            local conflict=""
            if ! _port_is_ours_or_free "$pvite" "$workspace"; then
                conflict="vite $pvite (pid ${OCCUPIER_PID:-unknown})"
            elif ! port_is_free "$papi"; then
                conflict="vite-internal $papi (pid $(lsof -ti :"$papi" -sTCP:LISTEN 2>/dev/null | head -1))"
            fi
            if [ -n "$conflict" ]; then
                echo "ERROR: pinned port for workspace '$workspace' is occupied: $conflict" >&2
                echo "       Source: $source (vite=$pvite, vite-internal=$papi)." >&2
                echo "       Free that port or change the pin in $workspace/lucidos.toml — refusing to walk forward off a pinned port." >&2
                return 1
            fi
            _finalize_ports "$workspace" "$offset" "$source" ""
            return $?
            ;;
    esac

    # Walk forward until api + vite are free (or held by us). PG is deliberately
    # NOT gated here: a PG-port collision (e.g. a sibling workspace's sticky
    # Docker container squatting our nominal 5432+offset slot) must never drift
    # the user-facing vite/api ports off their lucidos.toml pin. PG resolves its
    # own free port in setup_postgres (`_start_postgres_container`), independent
    # of this offset, and an existing container's actual published port is reused
    # over the nominal one — so an established workspace keeps its vite port even
    # when its PG slot is taken.
    local steps=0
    while :; do
        local cand_vite=$(( 5173 + offset ))
        local cand_api=$(( 3000 + offset ))

        if _port_is_ours_or_free "$cand_vite" "$workspace" \
            && _port_is_ours_or_free "$cand_api" "$workspace"; then
            break
        fi
        # OCCUPIER_PID is set by the failing check so we can name the
        # squatter without re-running lsof.
        local cmd=""
        [ -n "$OCCUPIER_PID" ] && cmd=$(ps -p "$OCCUPIER_PID" -o comm= 2>/dev/null | tr -d ' ' || true)
        echo "[ports] port $cand_vite/$cand_api occupied${cmd:+ by $cmd (pid $OCCUPIER_PID)}, walking forward" >&2

        offset=$(( offset + 1 ))
        steps=$(( steps + 1 ))
        if [ "$steps" -gt "$LUCIDOS_PORT_WALK_LIMIT" ]; then
            echo "ERROR: walked $LUCIDOS_PORT_WALK_LIMIT offsets without finding a free port pair" >&2
            return 1
        fi
    done

    _finalize_ports "$workspace" "$offset" "$source" ""
}
