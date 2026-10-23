#!/bin/bash
# Shared functions for Lucidos dev scripts (web-dev.sh, tauri-dev.sh).
# All functions operate on shared global variables set by earlier functions.
#
# Expected call order:
#   parse_dev_args "$@"
#   resolve_workspace
#   allocate_ports "$WORKSPACE"   (from lib/ports.sh)
#   detect_tls
#   setup_postgres
#   kill_stale_processes
#   build_or_find_engine
#   swap_ports
#   start_engine
#   start_vite
#   show_banner "web"|"tauri"

# Globals set by sourcing script before sourcing this file:
#   PROJECT_DIR, FRONTEND_DIR
# Globals set by callers:
#   SCRIPT_NAME  — basename of the calling script (for usage messages)

# Sourced here rather than left to the caller: not every caller of
# `setup_postgres` goes through preflight.sh (decommission-legacy-postgres.sh and
# lib/e2e.sh don't), and the Docker-state vocabulary has to be available wherever
# a `docker` call is about to happen. Include-guarded, so a caller that already
# sourced it pays nothing.
WORKSPACE_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/docker.sh
source "$WORKSPACE_LIB_DIR/docker.sh"
# For `ports_file_set`, which detect_tls and swap_ports both write through, and
# for the flock helpers it sources. The entry scripts source this first anyway,
# so re-sourcing only redefines the same functions. What it buys is that a
# caller of workspace.sh alone still has them.
# shellcheck source=scripts/lib/ports.sh
source "$WORKSPACE_LIB_DIR/ports.sh"

# The agent run marker (ADR 0391). A coding agent's turn-end sweep reaps every
# process carrying its marker, so a daemon meant to outlive the launch is
# exec'd without it. The engine pins this name to its own constant in a test.
AGENT_RUN_ENV="LUCIDOS_AGENT_RUN"
# For `proc_env_value`, which `engine_gateway_port` reads a running engine's own
# environment with. Pure parsing, no host call, so sourcing it costs nothing.
# shellcheck source=scripts/lib/proc_env.sh
source "$WORKSPACE_LIB_DIR/proc_env.sh"
# The ports, names and paths this file shares with the Rust crates.
# shellcheck source=scripts/lib/workspace_constants.sh
source "$WORKSPACE_LIB_DIR/workspace_constants.sh"
# shellcheck source=scripts/lib/pg_secret.sh
source "$WORKSPACE_LIB_DIR/pg_secret.sh"
# shellcheck source=scripts/lib/storage_root.sh
source "$WORKSPACE_LIB_DIR/storage_root.sh"
# For `prune_stale_debug_objects`, which every engine build runs (ADR 0408).
# shellcheck source=scripts/lib/stale_objects.sh
source "$WORKSPACE_LIB_DIR/stale_objects.sh"

# ── path_is_in_cc_worktree ──────────────────────────────────────────────
# True (exit 0) when $1 lies inside a coding-agent worktree — one of the
# `<workspace>/.lucidos/worktrees/<thread>/` copies the engine creates per
# coding-agent thread.
#
# Deliberately a pure string test with no `stat`: the case this exists to catch
# is a stack still running out of an ORPHANED worktree (pruned from git's
# registry, directory possibly already gone), so requiring the path to exist
# would make the guard fail open exactly when it matters most.
path_is_in_cc_worktree() {
    case "$1" in
        */"$WORKTREES_SUBPATH"/*|*/"$WORKTREES_SUBPATH") return 0 ;;
        *) return 1 ;;
    esac
}

# ── assert_stack_not_worktree_pinned ────────────────────────────────────
# Refuse to launch a long-lived stack whose checkout is a coding-agent worktree.
# Takes the checkout dir (PROJECT_DIR); returns 0 when it's fine to proceed and
# exits non-zero with an actionable message otherwise.
#
# Why this is fatal rather than auto-corrected: re-pointing the invocation at
# "the real checkout" means GUESSING which clone is canonical (a machine can have
# several), and silently running different code than the operator invoked is the
# same class of surprise as the bug being prevented. So: refuse, and name the
# command to run instead.
#
# Why a worktree-rooted stack is broken (the 2026-07-26 incident): a worktree is
# a throwaway copy pinned to ONE commit, and it contains a full `scripts/` tree,
# so `PROJECT_DIR="$(dirname "$SCRIPT_DIR")"` silently resolves there. The stack
# then serves that commit's engine binary and `dist/` forever — the
# checkout-level `vite build --watch` republishes the REAL checkout's `dist/`,
# which this stack never reads, so every frontend-only Apply looks like it did
# nothing. Worse, the gateway inherits LUCIDOS_STATIC_DIR / LUCIDOS_ENGINE_BIN
# into every engine it spawns and can re-exec onto a worktree-built binary, so
# the pin survives restarts and re-establishes itself.
#
# `LUCIDOS_ALLOW_WORKTREE_STACK=1` opts out — set by the e2e harness
# (scripts/lib/e2e.sh), whose workspace is disposable and whose whole point is
# exercising the checkout it was invoked from. The opt-in is deliberately an
# explicit env var rather than a workspace-name check (`e2e-test`): a name test
# fails OPEN if the name ever changes, and is invisible at the call site.
#
# `$2` = scope. **`gateway` ignores the opt-out entirely**, and that asymmetry is
# the whole point: the danger is not "a worktree", it is a MACHINE-GLOBAL daemon
# rooted in one. `run_gateway_supervised` traps SIGHUP/SIGINT/SIGTERM and is
# `disown`ed precisely so the gateway outlives the launching shell, and a `-b`
# run STOPS the existing gateway and relaunches it from the invoking checkout
# ("Stopping existing gateway for rebuild"). So `web-dev.sh -w e2e-test -b` from a
# coding-agent worktree — which the CC instructions used to recommend — kills the
# user's gateway and replaces it with one pinned to a throwaway checkout, which
# then adopts every workspace and serves them all its frozen dist/. That IS the
# 2026-07-26 incident, and no opt-in should be able to buy it.
#
# The e2e harness is unaffected: `scripts/lib/e2e.sh` calls `start_engine`
# directly (direct-engine model, ADR 0014), so it only ever hits the `stack`
# scope, where the opt-out applies. Its own session gateway (ADR 0385) is
# refused from a worktree before it starts.
assert_stack_not_worktree_pinned() {
    local project_dir="$1"
    local scope="${2:-stack}"
    path_is_in_cc_worktree "$project_dir" || return 0
    if [ "$scope" != "gateway" ] && [ "${LUCIDOS_ALLOW_WORKTREE_STACK:-}" = "1" ]; then
        return 0
    fi

    # A linked worktree's `.git` is a FILE holding
    # `gitdir: <main>/.git/worktrees/<name>` — so when it's still readable we can
    # name the real checkout instead of making the operator work it out.
    local real=""
    if [ -f "$project_dir/.git" ]; then
        real="$(sed -n 's/^gitdir: //p' "$project_dir/.git" 2>/dev/null \
                | sed 's#/\.git/worktrees/.*##')"
    fi
    [ -n "$real" ] || real="<your lucidos checkout>"

    if [ "$scope" = "gateway" ]; then
        cat >&2 <<EOF

ERROR: refusing to start the machine-global gateway from a coding-agent worktree.

  checkout: $project_dir

The gateway is ONE daemon per machine, disowned and signal-trapped so it outlives
the shell that launched it — and \`-b\` stops the running one and relaunches it
from whatever checkout invoked it. Rooted in a worktree it would outlive this
session, adopt every workspace, and serve them all a dist/ frozen at this
worktree's commit, while inheriting these paths into every engine it spawns. That
is how a frontend Apply silently does nothing for hours.

LUCIDOS_ALLOW_WORKTREE_STACK does NOT apply here — no test is worth replacing the
user's gateway with a throwaway one.

Start it from the real checkout:

  cd $real
  ./scripts/web-dev.sh -w <workspace> -b

To exercise THIS worktree's code, run the e2e scripts — they build the engine and
boot their own gateway-less, session-scoped engine, so no start step is needed:

  ./scripts/e2e.sh                 # full API + browser
  ./scripts/e2e-api.sh             # one suite

For a deliberate gateway-less run of some OTHER workspace from here, both opt-ins
are required (LUCIDOS_NO_GATEWAY alone still hits the worktree guard):

  LUCIDOS_NO_GATEWAY=1 LUCIDOS_ALLOW_WORKTREE_STACK=1 ./scripts/web-dev.sh -w <workspace> -b

EOF
        return 1
    fi

    cat >&2 <<EOF

ERROR: refusing to start a Lucidos stack from a coding-agent worktree.

  checkout: $project_dir

A worktree is a throwaway copy pinned to one commit. A stack launched from one
serves that commit's engine binary and frontend dist/ forever — the shared
\`vite build --watch\` republishes the real checkout's dist/, which this stack
never reads, so every frontend Apply silently appears to do nothing.

Start it from the real checkout instead:

  cd $real
  ./scripts/web-dev.sh -w <workspace> -b

If this really is a disposable e2e / session-local stack, opt in explicitly:

  LUCIDOS_ALLOW_WORKTREE_STACK=1 <your command>

EOF
    return 1
}

# ── parse_dev_args ──────────────────────────────────────────────────────
# Parse -w, -b, -r, -h flags. Sets WORKSPACE, BUILD, RELEASE.
parse_dev_args() {
    WORKSPACE="${LUCIDOS_WORKSPACE:-}"
    BUILD=""
    RELEASE=""
    ENGINE_ONLY=""
    # Build-only mode: rebuild the on-disk engine binary in the background WITHOUT
    # killing or respawning the running engine. Used by the Apply-triggered
    # background rebuild that powers the "new version available" surface (the
    # engine spawns `web-dev.sh --engine-build`; the disruptive switch happens
    # separately, via the /restart control call). See
    # docs/plans/2026-07-01-new-engine-version-switch-flow.md.
    ENGINE_BUILD_ONLY=""
    # shellcheck disable=SC2034 # read by scripts/web-dev.sh after it calls this parser
    FOLLOW_LOG=""
    while [[ $# -gt 0 ]]; do
        # A directive cannot sit on an individual case branch, so it goes here:
        # FOLLOW_LOG is set below and read by scripts/web-dev.sh after this parser
        # returns. Every other variable this case assigns has an in-file reader.
        # shellcheck disable=SC2034
        case $1 in
            -w|--workspace) WORKSPACE="$2"; shift 2 ;;
            -b|--build) BUILD="1"; shift ;;
            -r|--release) RELEASE="1"; shift ;;
            -f|--follow) FOLLOW_LOG="1"; shift ;;
            --built) shift ;;   # accepted for back-compat: the frontend is always built now
            --engine-only) ENGINE_ONLY="1"; BUILD="1"; shift ;;
            --engine-build) ENGINE_BUILD_ONLY="1"; BUILD="1"; shift ;;
            -h|--help)
                echo "Usage: $SCRIPT_NAME -w <workspace> [OPTIONS]"
                echo ""
                echo "Options:"
                echo "  -w, --workspace DIR   Workspace directory or name (required)"
                echo "  -b, --build           Build engine + gateway before starting"
                echo "  -r, --release         Build in release mode (slower build, faster runtime)"
                echo "  -f, --follow          Tail the engine log after startup (default: exit after ready)"
                echo "  --engine-only         Rebuild and restart only the engine (skip Vite, keep parent scripts)"
                echo "  -h, --help            Show this help"
                echo ""
                echo "The engine serves the built frontend (dist/) directly (ADR 0014);"
                echo "\`vite build --watch\` rebuilds it on change. There is no live dev server."
                echo ""
                echo "Examples:"
                echo "  $SCRIPT_NAME -w dev               # ~/workspaces/dev"
                echo "  $SCRIPT_NAME -w myws -b           # ~/workspaces/myws, build first"
                echo "  $SCRIPT_NAME -w dev -f             # start and tail the engine log"
                echo "  $SCRIPT_NAME -w /some/path -b -r  # absolute path, release build"
                exit 0
                ;;
            *) echo "Unknown option: $1"; exit 1 ;;
        esac
    done

    if [ -z "$WORKSPACE" ]; then
        echo "Error: No workspace specified."
        echo ""
        echo "Usage: $SCRIPT_NAME -w <workspace>"
        echo ""
        echo "Examples:"
        echo "  $SCRIPT_NAME -w dev"
        echo "  $SCRIPT_NAME -w ~/workspaces/myws"
        exit 1
    fi
}

# ── resolve_workspace_path ──────────────────────────────────────────────
# Pure resolver: expand bare name → ~/workspaces/<name>, canonicalize.
# Reads & writes $WORKSPACE. Returns 1 on missing workspace. No side effects.
# Use from read-only scripts (stop, status, tail) so unresolvable names
# error loudly instead of being silently treated as "nothing to do".
resolve_workspace_path() {
    if [[ "$WORKSPACE" != */* ]]; then
        WORKSPACE="$HOME/workspaces/$WORKSPACE"
    fi
    if [ ! -d "$WORKSPACE" ]; then
        echo "Error: Workspace not found: $WORKSPACE" >&2
        return 1
    fi
    WORKSPACE="$(cd "$WORKSPACE" && pwd)"
}

# ── resolve_workspace ───────────────────────────────────────────────────
# Same name resolution as resolve_workspace_path, plus the side effects
# needed to *start* a workspace: creates the workspace dir + subdirs if
# missing, and sets ENGINE_LOG / PG_NAME.
# Use from start scripts (web-dev.sh, tauri-dev.sh, e2e). For stop / status
# / tail, use resolve_workspace_path instead.
resolve_workspace() {
    # Bare name (no /) → ~/workspaces/<name>
    if [[ "$WORKSPACE" != */* ]]; then
        WORKSPACE="$HOME/workspaces/$WORKSPACE"
    fi

    # Create if needed, then resolve to absolute path.
    if [ ! -d "$WORKSPACE" ]; then
        echo "Creating workspace: $WORKSPACE"
    fi
    ensure_storage_root "$WORKSPACE"
    WORKSPACE="$(cd "$WORKSPACE" && pwd)"

    # One-time rebrand migration. Old name spelled via variable so this block
    # survives future bulk renames.
    local _old="cognos"
    if [ -d "$WORKSPACE/.${_old}" ]; then
        # If a prior broken run created an empty .lucidos/ stub (mkdir -p ran
        # before the engine crashed on auth), it has only runtime files
        # (engine.pid/log/ports) and no subdirectories. Real workspace state
        # always contains subdirs (worktrees/, browser-profile/, exhaust/...).
        # Stash the stub aside (never delete) so the real .cognos/ data wins.
        if [ -d "$WORKSPACE/.lucidos" ]; then
            local subdir_count
            subdir_count=$(find "$WORKSPACE/.lucidos" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
            if [ "$subdir_count" = "0" ]; then
                local stash
                stash="$WORKSPACE/.lucidos.stale-$(date +%Y%m%d%H%M%S)"
                echo "Stashing stub $WORKSPACE/.lucidos → $(basename "$stash") so .${_old}/ migration can proceed"
                mv "$WORKSPACE/.lucidos" "$stash"
            else
                echo "ERROR: both $WORKSPACE/.${_old}/ and $WORKSPACE/.lucidos/ contain real state — manual merge required" >&2
                return 1
            fi
        fi
        echo "Migrating $WORKSPACE/.${_old} → $WORKSPACE/.lucidos (one-time rebrand)"
        mv "$WORKSPACE/.${_old}" "$WORKSPACE/.lucidos"
        # Each CC worktree's .git file points at <repo>/.git/worktrees/<id>/, and
        # that gitdir file points back at the worktree's path. The mv broke that
        # back-pointer; `git worktree repair` rewrites the gitdir file to the
        # new path. No-op if .lucidos/worktrees/ is empty.
        if [ -d "$WORKSPACE/.lucidos/worktrees" ]; then
            local wt
            for wt in "$WORKSPACE/.lucidos/worktrees"/*/; do
                [ -d "$wt" ] || continue
                git -C "$wt" worktree repair >/dev/null 2>&1 || true
            done
        fi
    fi
    if [ -f "$WORKSPACE/.${_old}-workspace" ] && [ ! -f "$WORKSPACE/.lucidos-workspace" ]; then
        mv "$WORKSPACE/.${_old}-workspace" "$WORKSPACE/.lucidos-workspace"
    fi
    # Repo-root marker (default-workspace pointer consumed by start.sh).
    if [ -n "$PROJECT_DIR" ] && [ -f "$PROJECT_DIR/.${_old}-workspace" ] && [ ! -f "$PROJECT_DIR/.lucidos-workspace" ]; then
        mv "$PROJECT_DIR/.${_old}-workspace" "$PROJECT_DIR/.lucidos-workspace"
    fi

    # Ensure workspace directories exist.
    # PGDATA is no longer a host directory. Steady state is one shared Docker
    # volume (`lucidos-pg-data-shared`) with one database per workspace; legacy
    # `lucidos-pg-data-$PG_NAME` volumes are migration sources only.
    mkdir -p "$WORKSPACE/artifacts"
    mkdir -p "$WORKSPACE/.lucidos"

    # Workspace-scoped state files
    # The build-watch is NOT workspace-scoped: it produces the SHARED
    # crates/lucidos-app/dist/ that every workspace of this checkout serves, so
    # it is a checkout-level singleton (start_frontend_built, below).
    ENGINE_LOG="$WORKSPACE/.lucidos/engine.log"

    # Compute a short name for the postgres container from workspace path
    PG_NAME=$(printf '%s' "$WORKSPACE" | cksum | awk '{print $1}')
}

# ── detect_tls ──────────────────────────────────────────────────────────
# Check for TLS certs. Checks .certs/ dir first, then falls back to
# LUCIDOS_TLS_CERT/KEY env vars (needed in worktrees where .certs/ is gitignored).
# Sets PROTO, exports LUCIDOS_TLS_CERT/KEY. Persists PROTO= to the ports
# file so the CLI and cross-workspace callers know the protocol. That is only
# a first guess: once an engine answers, adopt_engine_scheme or
# record_engine_scheme replace it with the scheme the engine serves.
detect_tls() {
    local cert_dir="$PROJECT_DIR/.certs"
    if [ -f "$cert_dir/cert.pem" ] && [ -f "$cert_dir/key.pem" ]; then
        export LUCIDOS_TLS_CERT="$cert_dir/cert.pem"
        export LUCIDOS_TLS_KEY="$cert_dir/key.pem"
        PROTO="https"
    elif [ -f "$LUCIDOS_TLS_CERT" ] && [ -f "$LUCIDOS_TLS_KEY" ]; then
        PROTO="https"
    else
        PROTO="http"
    fi
    # Set rather than appended, and whether or not the file exists yet. An
    # append stacked a duplicate on every launch, and the existence guard meant
    # a launch that reached here first recorded nothing at all. Key-preserving,
    # so the port and Postgres keys the other writers own survive
    # (ports_file_set in lib/ports.sh says why they need to).
    ports_file_set "$WORKSPACE/.lucidos/ports" "PROTO=$PROTO"
}

# ── setup_postgres ──────────────────────────────────────────────────────
# Start/verify the ONE shared Docker Postgres cluster and ensure this
# workspace's database exists. Legacy per-workspace containers/volumes are kept
# intact; if present and this shared database has not been verified, they are
# dumped/restored into the shared cluster first.
shared_pg_container() { echo "${LUCIDOS_SHARED_PG_CONTAINER:-$SHARED_DOCKER_CONTAINER}"; }
shared_pg_volume()    { echo "${LUCIDOS_SHARED_PG_VOLUME:-$SHARED_DOCKER_VOLUME}"; }

# A native Postgres already listening on this loopback port replaces the Docker
# cluster. E2E GitHub mode sets it on macOS runners, which have no Docker.
external_pg_port() { echo "${LUCIDOS_EXTERNAL_PG_PORT:-}"; }

# shared_pg_psql <db> <psql args...>: psql against the shared cluster, wherever
# it runs. The one way the e2e path reaches Postgres.
shared_pg_psql() {
    local db="$1"; shift
    local port
    port="$(external_pg_port)"
    if [ -n "$port" ]; then
        PGPASSWORD="$(shared_pg_password)" psql -h 127.0.0.1 -p "$port" -U "$PG_USER" -d "$db" "$@"
    else
        docker exec "$(shared_pg_container)" psql -U "$PG_USER" -d "$db" "$@"
    fi
}

workspace_database_name() {
    local id
    id="$(workspace_slug)"
    echo "lucidos_$id"
}

# Carries the secret, so it goes only into a child's environment, never argv
# or a file another user can read.
workspace_database_url() {
    echo "postgres://$PG_USER:$(shared_pg_password)@localhost:$PG_PORT/$(workspace_database_name)"
}

_shared_pg_ident() {
    local name="$1"
    if [[ ! "$name" =~ ^[a-z0-9_-]+$ ]]; then
        echo "ERROR: invalid shared Postgres database name: $name" >&2
        return 1
    fi
    printf '"%s"' "$name"
}

_shared_pg_literal() {
    local value="$1"
    value=${value//\'/\'\'}
    printf "'%s'" "$value"
}

setup_postgres() {
    export LUCIDOS_WORKSPACE="$WORKSPACE"

    if [ -n "$(external_pg_port)" ]; then
        PG_PORT="$(external_pg_port)"
        export PG_PORT LUCIDOS_PG_PORT="$PG_PORT"
        _wait_for_external_postgres || return 1
        _ensure_shared_workspace_database "$(workspace_database_name)" || return 1
        return 0
    fi
    export LUCIDOS_PG_PORT="$PG_PORT"

    # Re-probe rather than trust the launch preflight. Minutes of building can
    # sit between the two (and lib/e2e.sh reaches here without a preflight at
    # all), so a daemon that went away in between would otherwise surface as a
    # raw `docker inspect`/`docker run` failure with no cause named.
    report_docker_daemon_if_down || return 1

    if _legacy_postgres_exists; then
        _migrate_postgres_if_needed
        _migrate_postgres_volume_if_needed
    fi

    _ensure_shared_postgres_container || return 1
    _migrate_workspace_postgres_to_shared_if_needed || return 1
    _ensure_shared_workspace_database "$(workspace_database_name)" || return 1
}

_legacy_postgres_exists() {
    docker inspect "lucidos-pg-$PG_NAME" >/dev/null 2>&1 && return 0
    docker volume inspect "lucidos-pg-data-$PG_NAME" >/dev/null 2>&1 && return 0
    [ -f "$WORKSPACE/data/postgres/PG_VERSION" ] && return 0
    return 1
}

_pg_port_is_shared_or_free() {
    local port="$1"
    port_is_free "$port" && return 0
    local container
    container=$(docker ps --filter "publish=$port" --format "{{.Names}}" 2>/dev/null | head -1)
    [ -n "$container" ] && [ "$container" = "$(shared_pg_container)" ]
}

_ensure_shared_postgres_container() {
    local container volume need_start=""
    container="$(shared_pg_container)"
    volume="$(shared_pg_volume)"

    if docker inspect "$container" >/dev/null 2>&1; then
        local container_status
        container_status=$(docker inspect --format='{{.State.Status}}' "$container" 2>/dev/null || echo "")
        if [ "$container_status" != "running" ]; then
            echo "Starting shared PostgreSQL container $container (was $container_status)"
            docker start "$container" >/dev/null || return 1
        else
            local actual_volume expected_volume
            actual_volume=$(docker inspect --format='{{range .Mounts}}{{if eq .Destination "/var/lib/postgresql"}}{{.Name}}{{end}}{{end}}' "$container" 2>/dev/null || echo "")
            expected_volume="$volume"
            if [ "$actual_volume" != "$expected_volume" ]; then
                echo "ERROR: shared PostgreSQL container $container uses unexpected volume '${actual_volume:-bind mount or empty}' (expected $expected_volume)" >&2
                return 1
            fi
        fi
    else
        need_start="1"
    fi

    if [ -n "$need_start" ]; then
        local pg_steps=0
        while ! _pg_port_is_shared_or_free "$PG_PORT"; do
            local squatter
            squatter=$(docker ps --filter "publish=$PG_PORT" --format "{{.Names}}" 2>/dev/null | head -1)
            echo "[ports] shared PG port $PG_PORT occupied${squatter:+ by container $squatter}, trying $(( PG_PORT + 1 ))" >&2
            PG_PORT=$(( PG_PORT + 1 ))
            pg_steps=$(( pg_steps + 1 ))
            if [ "$pg_steps" -gt 1000 ]; then
                echo "ERROR: could not find a free shared PostgreSQL port near $(( PG_PORT - pg_steps ))" >&2
                return 1
            fi
        done
        export PG_PORT
        export LUCIDOS_PG_PORT="$PG_PORT"

        echo "Starting shared PostgreSQL (port $PG_PORT, container $container)"
        docker volume create "$volume" >/dev/null || return 1
        # The gateway starts this same cluster when no script got there first, so
        # image, user and sizing come from workspace_constants.sh. The
        # gateway's postgres.rs says why the shared memory and connection limits
        # are raised.
        # POSTGRES_PASSWORD is named without a value, so docker reads it from
        # the environment and the secret stays out of argv.
        local password
        password="$(shared_pg_password)" || return 1
        if ! POSTGRES_PASSWORD="$password" docker run -d \
            --name "$container" \
            --restart unless-stopped \
            --shm-size="$PG_SHM_SIZE" \
            -p "127.0.0.1:$PG_PORT:5432" \
            -e POSTGRES_USER="$PG_USER" \
            -e POSTGRES_PASSWORD \
            -e POSTGRES_DB=postgres \
            -v "$volume:/var/lib/postgresql" \
            --label "lucidos.shared-postgres=true" \
            "$PG_IMAGE" \
            postgres -c max_connections="$PG_MAX_CONNECTIONS" >/dev/null; then
            # A daemon that died between the probe above and this call is BY FAR
            # the most common reason this fails, and `docker run`'s own message
            # for it names a socket path rather than the condition. Name the
            # condition first when that is what happened.
            report_docker_daemon_if_down || true
            echo "ERROR: failed to start shared PostgreSQL container $container" >&2
            return 1
        fi
    fi

    local actual_pg_port
    actual_pg_port=$(docker inspect --format='{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' "$container" 2>/dev/null || echo "")
    if [ -n "$actual_pg_port" ] && [ "$actual_pg_port" != "$PG_PORT" ]; then
        PG_PORT="$actual_pg_port"
        export PG_PORT
        export LUCIDOS_PG_PORT="$PG_PORT"
    fi

    # Readiness is NOT a single probe. On a FIRST run the image's entrypoint runs
    # initdb against a TEMPORARY server (listening on the unix socket only) and
    # then STOPS it before starting the real one. `pg_isready` answers yes during
    # that window, so a single success races the shutdown: the very next `psql`
    # gets "connection to server on socket ... failed: No such file or directory"
    # and the install dies. Seen on a clean Ubuntu CI runner pulling pg18 fresh.
    #
    # So require the probe to succeed CONSECUTIVELY, and probe over TCP (-h) the
    # way every real client connects — the temporary init server does not listen
    # on TCP at all, which is exactly what makes it distinguishable. A run that
    # briefly satisfies the probe and then drops resets the streak.
    # Probes five times a second, so a server that is already up costs well
    # under a second rather than three. Prints one dot a second.
    local ready_streak=0 ready_needed=3 tick
    echo -n "Waiting for shared PostgreSQL"
    for tick in $(seq 1 450); do
        if docker exec "$container" pg_isready -U lucidos -d postgres -h 127.0.0.1 -p 5432 >/dev/null 2>&1; then
            ready_streak=$((ready_streak + 1))
            if [ "$ready_streak" -ge "$ready_needed" ]; then
                echo " ready!"
                _set_shared_pg_password "$container"
                return $?
            fi
        elif [ "$ready_streak" -gt 0 ]; then
            # It answered, then stopped answering: that is the init server going
            # away. Start the count over rather than proceeding on a stale yes.
            ready_streak=0
        fi
        [ $((tick % 5)) -ne 0 ] || echo -n "."
        sleep 0.2
    done
    echo ""
    echo "ERROR: shared PostgreSQL did not become ready" >&2
    return 1
}

# Set the role's password to the minted secret, on every start. A cluster made
# before the secret existed still holds the old public password, and the image
# applies POSTGRES_PASSWORD only on first init. The statement goes in on stdin
# from the printf builtin, so the secret never reaches argv. The gateway does
# the same in `set_password_docker` (crates/lucidos-gateway/src/postgres.rs).
_set_shared_pg_password() {
    local container="$1" password
    password="$(shared_pg_password)" || return 1
    printf 'ALTER ROLE %s PASSWORD %s;\n' "$(_shared_pg_ident "$PG_USER")" "$(_shared_pg_literal "$password")" \
        | docker exec -i "$container" psql -U "$PG_USER" -d postgres -X -q -v ON_ERROR_STOP=1 >/dev/null
}

_shared_database_exists() {
    local db="$1"
    shared_pg_psql postgres -tAc \
        "SELECT 1 FROM pg_database WHERE datname=$(_shared_pg_literal "$db")" 2>/dev/null | grep -qx 1
}

_create_shared_database() {
    local db="$1" ident
    ident="$(_shared_pg_ident "$db")" || return 1
    shared_pg_psql postgres -v ON_ERROR_STOP=1 -c \
        "CREATE DATABASE $ident OWNER lucidos" >/dev/null
}

_drop_shared_database() {
    local db="$1" ident lit
    ident="$(_shared_pg_ident "$db")" || return 1
    lit="$(_shared_pg_literal "$db")"
    shared_pg_psql postgres -c \
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname=$lit AND pid <> pg_backend_pid()" >/dev/null 2>&1 || true
    shared_pg_psql postgres -c \
        "DROP DATABASE IF EXISTS $ident" >/dev/null
}

_verify_shared_pg_database() {
    local db="$1"
    shared_pg_psql "$db" -tAc "SELECT 1" 2>/dev/null | grep -qx 1
}

# The caller started the external server, so a short wait covers its startup.
_wait_for_external_postgres() {
    echo -n "Waiting for external PostgreSQL on port $(external_pg_port)"
    for _ in {1..30}; do
        if shared_pg_psql postgres -tAc "SELECT 1" >/dev/null 2>&1; then
            echo " ready!"
            return 0
        fi
        echo -n "."
        sleep 1
    done
    echo ""
    echo "ERROR: no PostgreSQL answered on LUCIDOS_EXTERNAL_PG_PORT=$(external_pg_port)" >&2
    return 1
}

_ensure_shared_workspace_database() {
    local db="$1"
    if _shared_database_exists "$db"; then
        _verify_shared_pg_database "$db" || return 1
        echo "PostgreSQL shared database ready: $db (port $PG_PORT)"
        return 0
    fi
    echo "Creating shared PostgreSQL database: $db"
    _create_shared_database "$db" || return 1
    _verify_shared_pg_database "$db" || return 1
}

_shared_pg_migration_marker() {
    echo "$WORKSPACE/.lucidos/shared-postgres-$(workspace_database_name).verified"
}

_legacy_pg_container() {
    echo "lucidos-pg-$PG_NAME"
}

_legacy_pg_volume() {
    echo "lucidos-pg-data-$PG_NAME"
}

_legacy_pg_volume_layout() {
    local volume="$1" old_ver
    if docker run --rm -v "$volume:/v:ro" pgvector/pgvector:pg18 \
        sh -c 'test -f /v/18/docker/PG_VERSION' >/dev/null 2>&1; then
        echo "parent:18"
        return 0
    fi
    old_ver=$(docker run --rm -v "$volume:/v:ro" pgvector/pgvector:pg18 \
        sh -c 'cat /v/PG_VERSION 2>/dev/null' | tr -d '[:space:]')
    if [[ "$old_ver" =~ ^[0-9]+$ ]]; then
        echo "root:$old_ver"
        return 0
    fi
    return 1
}

_start_legacy_postgres_from_volume() {
    local container volume layout image mount
    container="$(_legacy_pg_container)"
    volume="$(_legacy_pg_volume)"
    layout="$(_legacy_pg_volume_layout "$volume")" || {
        echo "ERROR: could not determine layout of legacy Postgres volume $volume; old data was not touched." >&2
        return 1
    }

    case "$layout" in
        parent:*)
            image="pgvector/pgvector:pg18"
            mount="/var/lib/postgresql"
            ;;
        root:*)
            image="pgvector/pgvector:pg${layout#root:}"
            mount="/var/lib/postgresql/data"
            ;;
        *)
            echo "ERROR: unsupported legacy Postgres volume layout '$layout'; old data was not touched." >&2
            return 1
            ;;
    esac

    echo "Recreating legacy PostgreSQL container $container from volume $volume for migration..."
    if ! docker run -d \
        --name "$container" \
        -v "$volume:$mount" \
        -e POSTGRES_USER=lucidos \
        -e POSTGRES_PASSWORD=lucidos \
        -e POSTGRES_DB=lucidos \
        --label "lucidos.legacy-postgres=true" \
        "$image" >/dev/null; then
        echo "ERROR: could not start legacy PostgreSQL container from $volume; old data was not touched." >&2
        return 1
    fi
}

_ensure_legacy_postgres_running() {
    local container
    container="$(_legacy_pg_container)"
    if ! docker inspect "$container" >/dev/null 2>&1; then
        if docker volume inspect "$(_legacy_pg_volume)" >/dev/null 2>&1; then
            _start_legacy_postgres_from_volume || return 1
            _migrate_postgres_if_needed || return 1
            container="$(_legacy_pg_container)"
        else
            return 1
        fi
    fi
    if [ "$(docker inspect --format='{{.State.Status}}' "$container" 2>/dev/null)" != "running" ]; then
        echo "Starting legacy PostgreSQL container $container for migration..."
        docker start "$container" >/dev/null || return 1
    fi
    echo -n "Waiting for legacy PostgreSQL"
    for _ in {1..120}; do
        if docker exec "$container" pg_isready -U lucidos -d lucidos >/dev/null 2>&1; then
            echo " ready!"
            return 0
        fi
        echo -n "."
        sleep 1
    done
    echo ""
    echo "ERROR: legacy PostgreSQL container $container did not become ready; old cluster left intact." >&2
    return 1
}

_legacy_pg_events_count() {
    local container
    container="$(_legacy_pg_container)"
    docker exec "$container" psql -U lucidos -d lucidos -tAc \
        "SELECT CASE WHEN to_regclass('public.events') IS NULL THEN 0 ELSE (SELECT count(*) FROM public.events) END" \
        2>/dev/null | tr -d '[:space:]'
}

_shared_pg_events_count() {
    local db="$1"
    docker exec "$(shared_pg_container)" psql -U lucidos -d "$db" -tAc \
        "SELECT CASE WHEN to_regclass('public.events') IS NULL THEN 0 ELSE (SELECT count(*) FROM public.events) END" \
        2>/dev/null | tr -d '[:space:]'
}

_write_shared_pg_migration_marker() {
    local db="$1" source="$2" marker
    marker="$(_shared_pg_migration_marker)"
    mkdir -p "$WORKSPACE/.lucidos"
    cat > "$marker" <<EOF
verified_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
database=$db
shared_container=$(shared_pg_container)
legacy_source=$source
EOF
}

_dump_legacy_postgres_to_file() {
    local dump="$1" container
    container="$(_legacy_pg_container)"
    docker exec "$container" rm -f /tmp/lucidos-shared-migrate.dump >/dev/null 2>&1 || true
    echo "Dumping legacy workspace database from $container ..."
    if ! docker exec "$container" pg_dump -U lucidos -d lucidos -Fc -f /tmp/lucidos-shared-migrate.dump; then
        echo "ERROR: pg_dump failed; old cluster left intact." >&2
        return 1
    fi
    if ! docker exec "$container" sh -c 'pg_restore -l /tmp/lucidos-shared-migrate.dump | grep -q .'; then
        echo "ERROR: dump archive verification failed; old cluster left intact." >&2
        return 1
    fi
    docker cp "$container:/tmp/lucidos-shared-migrate.dump" "$dump" >/dev/null || return 1
    docker exec "$container" rm -f /tmp/lucidos-shared-migrate.dump >/dev/null 2>&1 || true
}

_migrate_workspace_postgres_to_shared_if_needed() {
    local db marker container dump legacy_count shared_count
    db="$(workspace_database_name)"
    marker="$(_shared_pg_migration_marker)"
    container="$(_legacy_pg_container)"

    if ! _legacy_postgres_exists; then
        return 0
    fi

    # If the shared DB already exists, never overwrite it. Verify enough to make
    # decommission explicit and safe; if the legacy DB has more events, abort so
    # the user does not silently boot on an empty/newer target.
    if _shared_database_exists "$db"; then
        _verify_shared_pg_database "$db" || return 1
        if [ ! -f "$marker" ] && docker inspect "$container" >/dev/null 2>&1; then
            _ensure_legacy_postgres_running || return 1
            legacy_count="$(_legacy_pg_events_count)"
            shared_count="$(_shared_pg_events_count "$db")"
            if [ -n "$legacy_count" ] && [ -n "$shared_count" ] && [ "$legacy_count" -gt "$shared_count" ]; then
                echo "ERROR: shared database $db exists but has fewer events ($shared_count) than legacy $container ($legacy_count)." >&2
                echo "       Refusing to overwrite shared data. Old cluster left intact." >&2
                return 1
            fi
            _write_shared_pg_migration_marker "$db" "$container (pre-existing shared db)"
        fi
        return 0
    fi

    if ! docker inspect "$container" >/dev/null 2>&1 && \
       ! docker volume inspect "$(_legacy_pg_volume)" >/dev/null 2>&1; then
        return 0
    fi

    _ensure_legacy_postgres_running || return 1
    mkdir -p "$WORKSPACE/.lucidos"
    dump="$WORKSPACE/.lucidos/shared-postgres-$db.pending.dump"
    rm -f "$dump"

    echo ""
    echo "Migrating workspace PostgreSQL into shared cluster."
    echo "  Source: $container database lucidos"
    echo "  Target: $(shared_pg_container) database $db"
    _dump_legacy_postgres_to_file "$dump" || return 1

    echo "Creating shared database $db and restoring dump..."
    _create_shared_database "$db" || return 1
    if ! docker cp "$dump" "$(shared_pg_container):/tmp/lucidos-shared-restore.dump" >/dev/null; then
        _drop_shared_database "$db"
        echo "ERROR: could not copy dump into shared PostgreSQL container; old cluster left intact." >&2
        return 1
    fi
    if ! docker exec "$(shared_pg_container)" pg_restore -U lucidos --no-owner --no-privileges \
        --exit-on-error -d "$db" /tmp/lucidos-shared-restore.dump; then
        _drop_shared_database "$db"
        echo "ERROR: restore into shared PostgreSQL failed; old cluster left intact." >&2
        return 1
    fi
    docker exec "$(shared_pg_container)" rm -f /tmp/lucidos-shared-restore.dump >/dev/null 2>&1 || true
    _verify_shared_pg_database "$db" || { _drop_shared_database "$db"; return 1; }
    _write_shared_pg_migration_marker "$db" "$container"

    local archived
    archived="$WORKSPACE/.lucidos/shared-postgres-$db.restored-$(date +%Y%m%d%H%M%S).dump"
    mv "$dump" "$archived" 2>/dev/null || true
    echo "Shared PostgreSQL migration verified. Legacy container/volume kept for rollback:"
    echo "  $container / lucidos-pg-data-$PG_NAME"
    echo "Decommission explicitly after checking the workspace:"
    echo "  ./scripts/decommission-legacy-postgres.sh -w $WORKSPACE"
    echo ""
}

# One-time rebrand migration: container named with the legacy prefix and
# role/db `cognos` → container with role/db `lucidos`.
#
# Three independent renames (role, database, container) probed and applied
# separately so any partial state from a prior failed run is recovered:
#   1. Locate the container under either old or new name; start it if stopped.
#   2. Probe via psql (pg_isready does not authenticate) to learn which login
#      works (lucidos or cognos) and whether the lucidos database exists.
#   3. Rename the role if needed, via a temporary superuser (postgres refuses
#      ALTER USER on the session user). The role rename pair (RENAME + WITH
#      PASSWORD) runs in a transaction so a partial failure cannot leave the
#      role with an invalid password hash.
#   4. Rename the database if needed (must be a separate, single statement —
#      ALTER DATABASE ... RENAME cannot run inside a transaction).
#   5. Ensure the container is named with the new prefix.
_migrate_postgres_if_needed() {
    local _old="cognos"
    local old_container="${_old}-pg-$PG_NAME"
    local new_container="lucidos-pg-$PG_NAME"
    local tmp_role="_lucidos_migrate"
    local tmp_pw="_tmp"
    local container=""

    if docker inspect "$new_container" >/dev/null 2>&1; then
        container="$new_container"
    elif docker inspect "$old_container" >/dev/null 2>&1; then
        container="$old_container"
    else
        return 0
    fi

    if [ "$(docker inspect --format='{{.State.Status}}' "$container" 2>/dev/null)" != "running" ]; then
        docker start "$container" >/dev/null
    fi

    echo -n "Probing postgres in $container"
    local probe_user=""
    local recovery_announced=false
    for _ in {1..180}; do
        if docker exec "$container" psql -U lucidos -d postgres -tAc "SELECT 1" >/dev/null 2>&1; then
            probe_user="lucidos"
            break
        fi
        if docker exec "$container" psql -U "$_old" -d postgres -tAc "SELECT 1" >/dev/null 2>&1; then
            probe_user="$_old"
            break
        fi
        if ! $recovery_announced && docker logs --tail 50 "$container" 2>&1 | grep -qE "database system was interrupted|the database system is starting up"; then
            echo -n " (crash recovery in progress, may take a few minutes)"
            recovery_announced=true
        fi
        echo -n "."
        sleep 1
    done
    if [ -z "$probe_user" ]; then
        echo " ERROR" >&2
        echo "ERROR: postgres in $container did not become ready" >&2
        return 1
    fi
    echo " ready (login=$probe_user)"

    local has_lucidos_db
    has_lucidos_db=$(docker exec "$container" psql -U "$probe_user" -d postgres -tAc \
        "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname='lucidos')" 2>/dev/null | tr -d '[:space:]')

    # Fully migrated: role + db done. Just complete the container rename if
    # needed and clean up any temp role leaked from a prior failed run.
    if [ "$probe_user" = "lucidos" ] && [ "$has_lucidos_db" = "t" ]; then
        docker exec "$container" psql -U lucidos -d postgres -c \
            "DROP ROLE IF EXISTS $tmp_role" >/dev/null 2>&1 || true
        if [ "$container" = "$old_container" ]; then
            echo "Renaming container $old_container → $new_container"
            docker rename "$old_container" "$new_container"
        fi
        return 0
    fi

    echo "Migrating postgres role/db: ${_old} → lucidos (one-time rebrand)"

    # Step 1: rename the role if not yet done.
    if [ "$probe_user" = "$_old" ]; then
        if ! docker exec "$container" psql -U "$_old" -d postgres -c \
            "DROP ROLE IF EXISTS $tmp_role; CREATE ROLE $tmp_role WITH SUPERUSER LOGIN PASSWORD '$tmp_pw'" >/dev/null; then
            echo "ERROR: failed to create temp migration role" >&2
            return 1
        fi
        # RENAME invalidates any md5 password hash (it includes the username),
        # so RENAME and WITH PASSWORD must be atomic.
        if ! docker exec -e PGPASSWORD="$tmp_pw" "$container" psql -U "$tmp_role" -d postgres -v ON_ERROR_STOP=1 -c \
            "BEGIN; ALTER USER ${_old} RENAME TO lucidos; ALTER USER lucidos WITH PASSWORD 'lucidos'; COMMIT;"; then
            echo "ERROR: postgres role rename failed" >&2
            docker exec "$container" psql -U "$_old" -d postgres -c \
                "DROP ROLE IF EXISTS $tmp_role" >/dev/null 2>&1 || true
            return 1
        fi
        docker exec "$container" psql -U lucidos -d postgres -c \
            "DROP ROLE IF EXISTS $tmp_role" >/dev/null 2>&1 || \
            echo "WARN: temp migration role left in place (cleaned up on next run)" >&2
    fi

    # Step 2: rename the database if not yet done. Cannot run in a transaction.
    # web-dev.sh runs setup_postgres before kill_stale_processes, so a prior
    # engine may still hold connections to the old database — terminate them
    # first or the rename fails with "database is being accessed by other users".
    if [ "$has_lucidos_db" != "t" ]; then
        docker exec "$container" psql -U lucidos -d postgres -c \
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '${_old}' AND pid <> pg_backend_pid()" >/dev/null 2>&1 || true
        if ! docker exec "$container" psql -U lucidos -d postgres -c \
            "ALTER DATABASE ${_old} RENAME TO lucidos"; then
            echo "ERROR: postgres database rename failed" >&2
            return 1
        fi
    fi

    # Step 3: rename the container if not yet done.
    if [ "$container" = "$old_container" ]; then
        docker rename "$old_container" "$new_container"
    fi

    echo "Postgres role/db migrated: ${_old} → lucidos"
}

# One-time migration: workspaces created before the bind-mount → named-volume
# switch keep their PGDATA at <workspace>/data/postgres on the host. This copies
# the data into the new named volume so the running container can keep using
# the same database, then archives the host directory aside (never deleted).
#
# Idempotent: skipped if the named volume already exists or if there's no
# legacy bind-mount data to migrate.
_migrate_postgres_volume_if_needed() {
    local pg_data_dir="$WORKSPACE/data/postgres"
    local volume_name="lucidos-pg-data-$PG_NAME"
    local container="lucidos-pg-$PG_NAME"

    if docker volume inspect "$volume_name" >/dev/null 2>&1; then
        return 0
    fi
    if [ ! -f "$pg_data_dir/PG_VERSION" ]; then
        return 0
    fi

    echo ""
    echo "Migrating Postgres PGDATA from host bind mount to Docker named volume."
    echo "(One-time. Bind-mounted PGDATA crashes Docker Desktop's VM under sustained writes.)"
    echo "  Source:  $pg_data_dir"
    echo "  Target:  Docker volume $volume_name"

    # Release file locks before copy.
    if docker inspect "$container" >/dev/null 2>&1; then
        echo "Stopping container $container before migration..."
        docker rm -f "$container" >/dev/null 2>&1 || true
    fi

    if ! docker volume create "$volume_name" >/dev/null; then
        echo "ERROR: failed to create Docker volume $volume_name" >&2
        return 1
    fi

    # Run inside the postgres image so `chown postgres` resolves to the right
    # uid (999 in pgvector/pgvector:pg18; using the name keeps this correct if
    # the base image ever changes). Postgres refuses to start with PGDATA perms
    # other than 0700, and `cp -a` preserves the host's perms (often 0755 on
    # macOS bind mounts) — chmod here, not after the container starts.
    echo "Copying data into named volume..."
    if ! docker run --rm \
        -v "$pg_data_dir:/src:ro" \
        -v "$volume_name:/dst" \
        pgvector/pgvector:pg18 \
        sh -c 'cp -a /src/. /dst/ && chown -R postgres:postgres /dst && chmod 700 /dst'; then
        echo "ERROR: failed to copy PGDATA into volume $volume_name. Removing partial volume." >&2
        docker volume rm "$volume_name" >/dev/null 2>&1 || true
        return 1
    fi

    # Archive the old data dir aside (never delete — user can recover if anything
    # looks wrong after migration).
    local archive
    archive="$WORKSPACE/data/postgres.migrated-$(date +%Y%m%d%H%M%S)"
    if ! mv "$pg_data_dir" "$archive"; then
        echo "WARN: copied to volume but could not archive old dir at $pg_data_dir" >&2
    else
        echo "Old PGDATA preserved at: $archive"
        echo "(Safe to delete after verifying the engine starts and data looks correct.)"
        # The existing `data/postgres/` gitignore pattern is exact and does NOT
        # cover `data/postgres.migrated-*/`. Without this guard `git add .` (or
        # cpa) would happily commit the archived event store. Append once.
        local gi="$WORKSPACE/.gitignore"
        if [ -f "$gi" ] && ! grep -q '^data/postgres\.migrated-' "$gi" 2>/dev/null; then
            echo "data/postgres.migrated-*/" >> "$gi"
        fi
    fi
    echo ""
}

# ── _pid_in_list ────────────────────────────────────────────────────────
# True if pid $1 appears in the space-separated list $2.
_pid_in_list() {
    local needle="$1" hay="$2" p
    for p in $hay; do
        if [ "$p" = "$needle" ]; then return 0; fi
    done
    return 1
}

# ── _await_engine_port_released ─────────────────────────────────────────
# Poll until BOTH every pid in $2 has exited (kill -0 fails) AND port $1 is
# free (port_is_free), or $3 seconds elapse. Returns 0 if both conditions
# held before the deadline, 1 on timeout. Polls at 0.2s.
#
# Both conditions matter: a dead pid whose socket is still closing would let
# a port-only check return too early, and a freed port with the old process
# still alive means it could re-bind. SECONDS is integer wall-clock; the
# 0.2s poll keeps the loop responsive without busy-spinning.
_await_engine_port_released() {
    local port="$1"
    local pids="$2"
    local timeout_s="$3"
    local deadline=$(( SECONDS + timeout_s ))
    while (( SECONDS < deadline )); do
        local pid still_alive=""
        for pid in $pids; do
            if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
                still_alive=1
                break
            fi
        done
        if [ -z "$still_alive" ] && port_is_free "$port"; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

# ── wait_for_engine_shutdown ────────────────────────────────────────────
# Block until the old engine has fully released the engine port so the
# caller can build + launch the replacement without racing it onto an
# occupied socket (`Error: Os { code: 48, kind: AddrInUse }`, after which
# the replacement dies and the engine never recovers).
#
# The engine's graceful-shutdown budget is 10s (crates/lucidos-engine/src/
# main.rs), and draining an in-flight Claude Code session uses most of it —
# the previous fixed `sleep 1` returned long before the port was free. Poll
# up to 15s (comfortably past the 10s budget). If the deadline passes with a
# lucidos-engine still bound, escalate to SIGKILL — a wedged shutdown must
# not block the rebuild forever — then give the kernel a moment to reclaim
# the socket. Keeps the SIGUSR1-first convention: SIGKILL is the last resort,
# only after a full graceful window.
#
# Args: $1 = engine port (VITE_PORT — the engine binds it after swap_ports),
#       $2 = space-separated pids that were sent SIGUSR1,
#       $3 = overall timeout in seconds (default 15).
wait_for_engine_shutdown() {
    local port="$1"
    local pids="$2"
    local timeout_s="${3:-15}"

    if _await_engine_port_released "$port" "$pids" "$timeout_s"; then
        return 0
    fi

    # Deadline passed. Force-kill a wedged engine still bound to the port.
    if port_is_free "$port"; then
        return 0
    fi
    local occupant occupant_cmd
    occupant=$(lsof -ti :"$port" -sTCP:LISTEN 2>/dev/null | head -1 || true)
    if [ -n "$occupant" ]; then
        occupant_cmd=$(ps -p "$occupant" -o comm= 2>/dev/null || true)
        if [[ "$occupant_cmd" == *lucidos-engine* ]]; then
            # Stop the engine's supervisor BEFORE the SIGKILL. engine_supervisor.sh
            # treats a SIGKILL'd engine (exit 137) as an unexpected death and
            # respawns it right back onto this port — re-creating the AddrInUse
            # we're clearing. In --engine-only (in-app) restarts the old
            # supervisor is NOT torn down (the stale-dev-script sweep is skipped),
            # so it would win the race. The supervisor is the engine's parent;
            # its SIGTERM handler checks shutdown_requested before the exit-code
            # branch, so it forwards a final SIGUSR1 and exits without respawning.
            # (The engine itself only ever gets SIGUSR1 then SIGKILL — never
            # SIGTERM — so the SIGUSR1-stop convention is preserved.) Skip pid 1
            # (orphaned engine: no supervisor to stop) and our own pid.
            local sup_pid
            sup_pid=$(ps -p "$occupant" -o ppid= 2>/dev/null | tr -d ' ' || true)
            if [ -n "$sup_pid" ] && [ "$sup_pid" != "1" ] && [ "$sup_pid" != "$$" ]; then
                kill -TERM "$sup_pid" 2>/dev/null || true
            fi
            echo "Engine did not release port $port within ${timeout_s}s — force-killing PID $occupant..." >&2
            kill -KILL "$occupant" 2>/dev/null || true
        fi
    fi
    # Brief grace for the kernel to reclaim the socket after SIGKILL.
    if _await_engine_port_released "$port" "" 3; then
        return 0
    fi
    echo "WARNING: port $port still occupied after force-kill — the replacement engine may fail to bind." >&2
    return 1
}

# ── select_stale_dev_script_pids ────────────────────────────────────────
# Print the PIDs of dev-script processes belonging to THIS workspace.
#
# CRITICAL: match the SCRIPT PATH token, never a substring of the whole
# command line. `pgrep -f '<name>\.sh.*$WORKSPACE'` matches the phrase
# ANYWHERE in a process's argv, and a coding agent carried the engine's thread
# history inside a roughly 22 KB `--append-system-prompt` argument. Any thread
# quoting a dev script and then this workspace path became a kill candidate,
# and the caller sends `pkill -P` plus a SIGTERM, so it takes the session's
# children with it. Same class as ADR 0025's webkit reaper.
#
# A shell script's argv[0] is the interpreter, so the script path is argv[1].
# Testing that ONE token keeps `bash -c '<text naming web-dev.sh>'` out of
# scope, because its argv[1] is `-c`. Only after that token matches is the
# workspace path looked for in the remaining arguments.

# ── _dev_script_list_processes ──────────────────────────────────────────
# The one process listing the selector below samples. A test overrides it to feed
# synthetic rows, exactly as `webkit_reaper.sh` does. Keep it a seam: every row
# it emits becomes a `pkill -P` plus a `kill`, so a test that reached the real
# `ps` would aim those at the host.
_dev_script_list_processes() {
    ps -Aww -o pid=,command= 2>/dev/null
}

select_stale_dev_script_pids() {
    local pid argv0 argv1 rest script
    _dev_script_list_processes | while read -r pid argv0 argv1 rest; do
        case "$pid" in ''|*[!0-9]*) continue ;; esac
        [ "$pid" -le 1 ] && continue
        [ "$pid" = "$$" ] && continue
        # argv[0] is the script when it was exec'd through its own shebang, and
        # the interpreter when it was invoked as `bash scripts/web-dev.sh`.
        script="$argv0"
        case "${argv0##*/}" in
            bash|sh|zsh|dash) script="$argv1" ;;
        esac
        case "${script##*/}" in
            dev.sh|web-dev.sh|tauri-dev.sh) ;;
            *) continue ;;
        esac
        case "$argv1 $rest" in
            *"$WORKSPACE"*) ;;
            *) continue ;;
        esac
        # The engine's own background build is never stale: this launch does
        # not replace it, and a SIGTERM fails the build the user is waiting on.
        # Matched as a whole token, so a path containing the text still counts.
        case " $argv1 $rest " in
            *" --engine-build "*) continue ;;
        esac
        # Backstop for a matcher that ever broadens again: never signal another
        # workspace's live engine or an ancestor of this process.
        if command -v is_protected_host_pid >/dev/null 2>&1 && is_protected_host_pid "$pid"; then
            continue
        fi
        printf '%s\n' "$pid"
    done
}

# ── kill_stale_processes ────────────────────────────────────────────────
# Kill stale dev script processes and old frontend for this workspace.
# With -b: also kills the engine (need fresh build). Without -b: leaves
# a healthy engine running so multiple clients can share it.
kill_stale_processes() {
    local self_pid=$$
    local killed=""
    # PIDs we sent SIGUSR1 to that must fully exit before the caller builds +
    # launches the replacement engine (see wait_for_engine_shutdown below).
    local engine_pids_to_reap=""

    # In --engine-only mode, skip killing parent dev scripts (they manage Vite/Tauri)
    if [ -z "$ENGINE_ONLY" ]; then
        # Kill stale dev script processes for THIS workspace (excluding ourselves)
        local stale_pid
        while IFS= read -r stale_pid; do
            if [ -z "$stale_pid" ]; then continue; fi
            if [ "$stale_pid" = "$self_pid" ]; then continue; fi
            echo "Killing stale dev script for this workspace (PID $stale_pid)..."
            pkill -P "$stale_pid" 2>/dev/null || true
            kill "$stale_pid" 2>/dev/null || true
            killed=1
        done < <(select_stale_dev_script_pids)
    fi

    # Gateway-mode --engine-only (ADR 0014): leave the shared gateway AND this
    # engine alone. The gateway is on the fixed GATEWAY_PORT and the engine on
    # ENGINE_PORT (VITE_PORT, network-bound — dev topology §4); killing either
    # here would force a full gateway restart (disrupting peers). Instead
    # start_gateway reuses the live shared gateway and asks it to respawn just
    # THIS workspace's engine onto the freshly-built binary — a targeted Apply
    # restart. A full `-b` launch (no --engine-only) falls through and reclaims
    # the ports, replacing both.
    local skip_engine_kill=""
    if [ -n "$ENGINE_ONLY" ] && [ -z "${LUCIDOS_NO_GATEWAY:-}" ]; then
        skip_engine_kill="1"
    fi

    # With -b: kill existing engine so we start the freshly built one.
    # Use SIGUSR1, not SIGTERM — the engine ignores SIGTERM to survive
    # accidental `xargs kill` from CC subprocess test scripts (see
    # main.rs shutdown_signal). SIGUSR1 is the legitimate stop signal.
    if [ -n "$BUILD" ] && [ -z "$skip_engine_kill" ]; then
        local old_pid
        for old_pid in $(engine_pids_to_signal "$WORKSPACE"); do
            echo "Stopping existing engine for rebuild (PID $old_pid)..."
            kill -USR1 "$old_pid" 2>/dev/null || true
            engine_pids_to_reap="${engine_pids_to_reap:+$engine_pids_to_reap }$old_pid"
            killed=1
        done

        # Full `-b` rebuild in gateway mode (not --engine-only Apply, which is
        # skip_engine_kill): also stop the ONE shared gateway so the freshly built
        # `lucidos-gateway` binary is used and a stale/unhealthy gateway can't
        # squat the fixed gateway port. SIGUSR1 = graceful stop leaving its
        # engines for re-adoption; the fresh gateway re-adopts every running peer
        # (so peers survive with only a brief proxy gap) and the launcher then
        # starts this workspace's engine (killed above). The gateway is its own
        # binary (ADR 0014) — a different process name than the engine, so the
        # orphan-port reclaim below must match it too. (CC's Apply uses
        # --engine-only and does NOT take this branch, so it never disrupts peers.)
        if [ -z "${LUCIDOS_NO_GATEWAY:-}" ] && [ -f "$(gateway_pidfile)" ]; then
            local gw_old
            gw_old="$(cat "$(gateway_pidfile)" 2>/dev/null || true)"
            if [ -n "$gw_old" ] && kill -0 "$gw_old" 2>/dev/null; then
                echo "Stopping existing gateway for rebuild (PID $gw_old)..."
                kill -USR1 "$gw_old" 2>/dev/null || true
                engine_pids_to_reap="${engine_pids_to_reap:+$engine_pids_to_reap }$gw_old"
                killed=1
            fi
            rm -f "$(gateway_pidfile)"
        fi

        # Kill any orphan still holding VITE_PORT (= ENGINE_PORT). In the ADR 0014
        # dev topology the ENGINE binds this port directly; in legacy mode it's
        # also the engine; on the first `-b` after the old gateway-on-the-user-
        # port topology it could be an old `lucidos-engine --gateway` — hence the
        # `lucidos-gateway` match too. (The gateway's own port, GATEWAY_PORT, is
        # reclaimed by the pidfile kill above + the start_gateway safety net.)
        # Skip pids already signaled above (still draining); only touch our binaries.
        if ! port_is_free "$VITE_PORT"; then
            local occupant occupant_cmd
            occupant=$(lsof -ti :"$VITE_PORT" -sTCP:LISTEN 2>/dev/null | head -1 || true)
            if [ -n "$occupant" ] && ! _pid_in_list "$occupant" "$engine_pids_to_reap"; then
                occupant_cmd=$(ps -p "$occupant" -o comm= 2>/dev/null || true)
                if [[ "$occupant_cmd" == *lucidos-engine* || "$occupant_cmd" == *lucidos-gateway* ]]; then
                    echo "Killing orphaned $occupant_cmd on port $VITE_PORT (PID $occupant)..."
                    kill -USR1 "$occupant" 2>/dev/null || true
                    engine_pids_to_reap="${engine_pids_to_reap:+$engine_pids_to_reap }$occupant"
                    killed=1
                fi
            fi
        fi
    fi

    # The build-watch is a checkout-level singleton shared across workspaces,
    # so a per-workspace restart never touches it. start_frontend_built reuses a
    # running one.

    # Wait for killed processes to release their ports before the caller
    # builds + launches the replacements. The engine needs an explicit wait:
    # its graceful-shutdown budget is 10s (main.rs) and draining an in-flight
    # Claude Code session uses most of it, so a fixed `sleep 1` raced the
    # rebuild onto a still-bound port (Error: AddrInUse, engine never
    # recovered). wait_for_engine_shutdown blocks until the engine is gone AND
    # the port is free. A frontend-only kill (Vite releases its port almost
    # instantly on SIGTERM) keeps the short fixed wait.
    if [ -n "$engine_pids_to_reap" ]; then
        # Best-effort: `|| true` so an unrecoverable port (the rare case where
        # even SIGKILL doesn't free it, where wait_for_engine_shutdown returns
        # non-zero after warning) doesn't abort the caller under `set -e`. We
        # still want to proceed to the rebuild + start_engine, which surfaces
        # any bind error itself rather than dying silently here.
        wait_for_engine_shutdown "$VITE_PORT" "$engine_pids_to_reap" || true
    elif [ -n "$killed" ]; then
        sleep 1
    fi
}

# ── select_dev_windows ──────────────────────────────────────────────────
# Print the PID of every dev window running <window_bin>, this checkout's
# published dev window binary.
#
# Selection is on argv[0] alone (ADR 0025). The launcher starts the window by
# its resolved path, and the window's own re-exec keeps that path, so a window
# from another checkout, or a process that only mentions the path, never
# matches. A path holding a space is matched as a prefix ending in a space.
#
# Test seam: `_dev_window_ps_feed` prints `<pid> <args>` lines.
_dev_window_ps_feed() {
    ps -Ao pid=,args= 2>/dev/null || true
}

select_dev_windows() {
    local window_bin="$1" pid args
    while read -r pid args; do
        case "$args" in
            "$window_bin" | "$window_bin "*) printf '%s\n' "$pid" ;;
            *) ;;
        esac
    done < <(_dev_window_ps_feed)
}

# ── stop_dev_windows ────────────────────────────────────────────────────
# Stop this checkout's previous dev window, so a relaunch leaves one.
stop_dev_windows() {
    local window
    for window in $(select_dev_windows "$1"); do
        if command -v is_protected_host_pid >/dev/null 2>&1 && is_protected_host_pid "$window"; then
            continue
        fi
        echo "Stopping the previous dev window (PID $window)..."
        kill "$window" 2>/dev/null || true
    done
}

# ── Published launch binaries (ADR 0022) ────────────────────────────────
# `target/<profile>/lucidos-engine` is ONE output path that EVERY cargo variant
# in the checkout uplifts to, and the last writer wins: a workspace-scope
# `cargo test`, an e2e `--features e2e-test-hooks` build, a build whose build.rs
# ran two commits ago. Launching from it means a workspace can run — and can
# read back via `current_exe --build-id` — a binary that is not the one its own
# build produced (the 2026-07-26 downgrade/toast loop; see
# docs/plans/2026-07-27-launch-binary-published-per-variant.md).
#
# So a completed build PUBLISHES its outputs into
# `.launch/<profile>/<variant>/`, and that is what every launch path uses.
# The directory is written only by completed builds of the same profile AND
# feature variant, so it can neither hold another configuration's binary nor be
# relinked mid-life by a co-located peer's build. Cargo keeps uplifting to
# `target/<profile>/lucidos-engine`; nothing launches from it any more.
#
# Staying inside the CHECKOUT is load-bearing; staying inside `target/` is NOT
# (ADR 0063, which is why the dir is `.launch/`). Two things depend on the
# location and both need only "somewhere under the repo root": the engine
# resolves the checkout by walking `current_exe()`'s ancestors for
# `scripts/web-dev.sh` (`crates/lucidos-engine/src/paths.rs`), and ADR 0021's
# worktree refusal is a pure path test on `LUCIDOS_ENGINE_BIN`. A
# WORKSPACE-local staging dir would break the first and launder a worktree
# binary past the second, which is what ADR 0022 ruled out.

# Filesystem-safe component naming the cargo feature configuration the binaries
# were built with: `plain` for a default build, else the requested features.
# ENGINE_BUILD_FEATURES is the space-separated list the caller wants enabled on
# `lucidos-engine` (the e2e scripts set `e2e-test-hooks` so the engine compiles
# in the push-log stub and `GET /api/v1/_test/push-log`).
engine_build_variant_slug() {
    local features="${ENGINE_BUILD_FEATURES:-}" slug
    if [ -z "$features" ]; then
        echo "plain"
        return 0
    fi
    slug="$(printf '%s' "$features" | tr ' ,' '__' | tr -cd 'A-Za-z0-9_-')"
    # Nothing survived sanitizing (an exotic feature name): still give the build
    # its own directory rather than a nameless one that would collide with the
    # launch dir itself.
    [ -n "$slug" ] || slug="custom"
    echo "$slug"
}

# The cargo profile directory the build lands in: ENGINE_CARGO_PROFILE when a
# caller names one (GitHub e2e builds use `e2e`), else `release` when the caller
# asked for a release build, else `debug`.
engine_build_profile() {
    if [ -n "${ENGINE_CARGO_PROFILE:-}" ]; then
        echo "$ENGINE_CARGO_PROFILE"
    elif [ -n "${RELEASE:-}" ]; then
        echo "release"
    else
        echo "debug"
    fi
}

# Directory the launch binaries for a (profile, variant) pair are published to.
#
# Deliberately NOT under `target/`, and that is the whole point of the directory
# (ADR 0022 originally put it at `target/<profile>/launch/<variant>`; ADR 0063
# amending that records why). `cargo clean` removes `target/` wholesale, and the
# launch dir is where the running system's `lucidos` CLI lives: the engine finds
# it by walking up from its own exe (`find_lucidos_cli_dir`), pins a copy into
# `<workspace>/.lucidos/cli` (ADR 0512), and prepends that to PATH for every
# spawned coding-agent session and trigger subprocess. So
# a single `cargo clean` used to take the CLI out from under a running engine.
# On 2026-08-13 the nightly orchestrator ran one inline, and for the next eight
# hours every trigger that shells out to `lucidos` died with "No such file or
# directory", while `run_coding_agent` could not spawn the child that would have
# rebuilt it, because that spawn needs the same missing CLI.
#
# What ADR 0022 actually requires of this path is that it stay inside the
# CHECKOUT, and a checkout-local dot-dir keeps both properties that depend on
# it: `paths::repo_root_above` is a pure ancestor walk for the
# `scripts/web-dev.sh` marker, so it resolves from any depth; and ADR 0021's
# worktree refusal is a substring test for `/.lucidos/worktrees/`, which a
# worktree's own `.launch/` still satisfies. A WORKSPACE-local dir would break
# both, which is what that ADR ruled out.
launch_bin_dir() {
    local profile="${1:-$(engine_build_profile)}"
    local variant="${2:-$(engine_build_variant_slug)}"
    echo "$PROJECT_DIR/.launch/$profile/$variant"
}

# Publish one freshly built binary. Copies into the destination directory under
# a temp name, signs it THERE, and `mv -f`s it into place: a same-filesystem
# rename, so the path only ever holds a COMPLETE binary and a running process
# keeps its own inode. Any failure removes the temp file and leaves an
# already-published binary untouched — a build must never make the launch path
# missing or truncated (that would strand every co-located workspace behind "No
# engine binary found"). Returns non-zero when nothing was published.
#
# `$3 = sign` re-signs with the stable dev identity. It runs on the TEMP file,
# before the rename, and that ordering is load-bearing: `codesign --force`
# rewrites its target in place, so signing the already-published path would
# leave a peer spawning a half-rewritten binary — defeating the very atomicity
# this function exists for. The temp name is pid-unique and nothing launches
# from it, so it is the only mutable file in the whole publish. Signing a
# `*.tmp.<pid>` file is safe because `sign_engine_binary` passes an explicit
# `--identifier`, so the rebuild-stable Designated Requirement does not depend
# on the filename.
publish_launch_binary() {
    local src="$1" dst="$2" sign="${3:-}"
    [ -f "$src" ] || return 1
    mkdir -p "$(dirname "$dst")" || return 1
    local tmp="$dst.tmp.$$"
    # `cp -c` clones on APFS (near-free for a ~250 MB debug binary); plain `cp`
    # everywhere else. Never copy onto `$dst` directly — that would truncate a
    # binary a peer engine may be about to spawn.
    if ! { cp -c "$src" "$tmp" 2>/dev/null || cp "$src" "$tmp"; }; then
        rm -f "$tmp"
        return 1
    fi
    chmod +x "$tmp" 2>/dev/null || true
    # The mtime says when this file was published, not when cargo linked it. A
    # dev window compares it with its engine's start (ADR 0461).
    touch "$tmp" 2>/dev/null || true
    if [ "$sign" = "sign" ]; then
        sign_engine_binary "$tmp"
    fi
    if ! mv -f "$tmp" "$dst"; then
        rm -f "$tmp"
        return 1
    fi
}

# Delete publish temps left behind by a build that was KILLED mid-publish.
#
# `publish_launch_binary` removes its own temp on every failure it can observe,
# but the engine SIGKILLs the whole build process group when an explicit Rebuild
# restarts a build (`engine_version::BuildProcessGroupGuard`), and no trap
# catches SIGKILL. A kill landing inside the copy/sign window therefore strands
# a `*.tmp.<pid>` that nothing ever collects, and a debug engine binary is
# ~250 MB of it. Nothing launches from a temp, so this is disk hygiene rather
# than correctness.
#
# Scoped by the PID IN THE NAME, not by age. A temp whose shell is still alive
# belongs to a publish in flight, and a human `web-dev.sh -b` is deliberately
# not coordinated by the engine's build lock, so deleting one would break that
# build's rename. A dead pid cannot be publishing anything. A recycled pid just
# means the temp survives until the next publish, which is the safe direction.
prune_dead_launch_temps() {
    local dest_dir="$1" tmp pid
    for tmp in "$dest_dir"/*.tmp.*; do
        # Unmatched glob comes through literally; a temp may also vanish under us.
        [ -f "$tmp" ] || continue
        pid="${tmp##*.tmp.}"
        case "$pid" in
            '' | *[!0-9]*) continue ;;
        esac
        # `if` rather than `&&`, so a dead pid is a plain false and never trips
        # the caller's errexit.
        if kill -0 "$pid" 2>/dev/null; then
            continue
        fi
        rm -f "$tmp"
    done
}

# Publish the engine + gateway + CLI from cargo's uplift dir into the launch
# dir. Returns non-zero when the engine or the gateway could not be published.
# The engine + gateway are signed on the way in (see publish_launch_binary);
# the CLI is not, matching what the dev launcher has always signed.
publish_launch_binaries() {
    local src_dir="$1" dest_dir="$2"
    local rc=0
    prune_dead_launch_temps "$dest_dir"
    publish_launch_binary "$src_dir/lucidos-engine" "$dest_dir/lucidos-engine" sign || rc=1
    publish_launch_binary "$src_dir/lucidos-gateway" "$dest_dir/lucidos-gateway" sign || rc=1
    # The `lucidos` CLI must sit NEXT TO the engine: find_lucidos_cli_dir
    # (crates/lucidos-engine/src/runtime/lucidos_cli.rs) walks up from the
    # engine's exe dir looking for it, and the engine prepends that dir to PATH
    # for spawned coding-agent sessions. Non-fatal — the engine degrades to
    # skipping the lucidos-cli skill rather than failing to boot.
    publish_launch_binary "$src_dir/lucidos" "$dest_dir/lucidos" || true
    return $rc
}

# ── The dev window's launch binary (ADR 0461) ───────────────────────────
# tauri-dev.sh runs the desktop window from the launch dir, beside the engine
# and gateway. The build that publishes them publishes it too, so one switch
# moves all three. Only a checkout that has run a dev window builds it: its
# first published binary is the opt-in, so a worktree, an e2e build or a
# machine without Tauri's libraries never compiles the app.
dev_window_bin() {
    echo "$1/lucidos-app"
}

run_dev_window_cargo_build() {
    cargo_build_in_slot "dev window build" -p lucidos-app
}

# A running window re-execs onto a NEW file at its path after the next switch,
# so a rebuild with identical bytes must leave the published file alone.
publish_dev_window_binary() {
    local src="$1" dst="$2"
    if cmp -s "$src" "$dst"; then
        return 0
    fi
    publish_launch_binary "$src" "$dst"
}

# Build and publish the dev window alone, for the first launch on a checkout.
# Every later build compiles it with the engine, in build_or_find_engine.
build_dev_window() {
    local launch_dir="$1"
    echo ""
    echo "Building the dev window..."
    run_dev_window_cargo_build || return 1
    publish_dev_window_binary "$(dev_window_bin "$PROJECT_DIR/target/$(engine_build_profile)")" \
        "$(dev_window_bin "$launch_dir")"
}

# Classify a published binary's baked build id against the checkout's current
# HEAD: `current`, `stale`, or `unknown`.
#
# `stale` means the build did NOT produce a binary for the source that is on
# disk now — `build.rs` stamps the id when the build script RUNS, so a build
# that starts at commit N and finishes after an Apply moved main to N+1
# publishes an N binary. It is stale only if N..N+1 touched a file that needs a
# restart: a CSS-only move keeps it current. `unknown` (no git, unreadable/empty id, a no-git
# `src-…` id) is deliberately NOT a mismatch: the same asymmetry the engine's
# direction guard uses, so an unresolvable id never costs a rebuild.
published_build_state() {
    local bin="$1"
    local head id commit
    head="$(git -C "$PROJECT_DIR" rev-parse --short HEAD 2>/dev/null || true)"
    [ -n "$head" ] || { echo "unknown"; return 0; }
    id="$("$bin" --build-id 2>/dev/null || true)"
    [ -n "$id" ] || { echo "unknown"; return 0; }
    # `<sha>` for a clean tree, `<sha>-<diffhash>` when engine source is dirty.
    commit="${id%%-*}"
    case "$commit" in
        "" | src*) echo "unknown"; return 0 ;;
    esac
    # Compare as prefixes in BOTH directions: the two sides can be abbreviated
    # to different lengths (git widens the short sha as the object count grows),
    # and a plain `=` would then report a false mismatch and rebuild forever.
    case "$head" in "$commit"*) echo "current"; return 0 ;; esac
    case "$commit" in "$head"*) echo "current"; return 0 ;; esac
    # HEAD moved. The binary still serves it unless a file that needs a restart
    # changed, and the binary answers that with the engine's own classifier. A
    # binary that predates the flag prints its id instead, which stays stale.
    if [ "$("$bin" --build-id --source-state 2>/dev/null || true)" = "current" ]; then
        echo "current"
        return 0
    fi
    echo "stale"
}

# The cargo invocation itself. Extracted so the staleness retry below re-runs
# exactly the same build. lucidos-cli is built alongside the engine so the
# `lucidos` binary is published next to `lucidos-engine`; lucidos-gateway
# (ADR 0014) is the standalone front the dev launcher spawns.
#
# Runs under a *build slot* (ADR 0070), which is the ONE point every engine
# build passes through: the e2e harness, a human `web-dev.sh -b`, and the
# engine's own background rebuild, which reaches here via
# `web-dev.sh --engine-build`. That last one already holds the checkout build
# lock by the time it gets here, so the two are taken in the right order
# without either knowing about the other.
#
# Bootstrapping is the degrade path doing its job: this build is what PRODUCES
# the `lucidos` binary, so on a fresh checkout there is none and the wrapper
# runs cargo unrestricted. Every later build finds it and takes a slot.
run_engine_cargo_build() {
    local feature_args=()
    if [ -n "${ENGINE_BUILD_FEATURES:-}" ]; then
        feature_args=(--features "$ENGINE_BUILD_FEATURES")
    fi
    cargo_build_in_slot "engine build" \
        -p lucidos-engine "${feature_args[@]}" -p lucidos-gateway -p lucidos-cli
}

# `cargo build --locked <args>` under a build slot, in the profile
# engine_build_profile picks. The label names the build in the slot's queue.
cargo_build_in_slot() {
    local label="$1" profile
    shift
    local slot="$PROJECT_DIR/scripts/with-build-slot.sh"
    profile="$(engine_build_profile)"
    case "$profile" in
        debug) "$slot" --label "$label" -- cargo build --locked "$@" ;;
        *) "$slot" --label "$label ($profile)" -- cargo build --locked "$@" --profile "$profile" ;;
    esac
}

# Compile every launch binary: the engine, gateway and CLI, then the dev window
# when <with_window> is set. All of it compiles before anything is published,
# so a failure leaves the previous version whole and a switch never finds a
# half-built one (ADR 0461).
run_launch_cargo_builds() {
    run_engine_cargo_build || return 1
    if [ -n "$1" ]; then
        run_dev_window_cargo_build || return 1
    fi
}

# Publish every launch binary run_launch_cargo_builds compiled. Non-zero only
# when the dev window could not be published. The engine half keeps its own
# fallback in build_or_find_engine.
publish_all_launch_binaries() {
    local uplift_dir="$1" launch_dir="$2" with_window="$3"
    publish_launch_binaries "$uplift_dir" "$launch_dir" || true
    # After the engine: an engine started after the window's publish time must
    # already run this build, or the window would move ahead of it.
    if [ -n "$with_window" ]; then
        publish_dev_window_binary "$(dev_window_bin "$uplift_dir")" "$(dev_window_bin "$launch_dir")" \
            || return 1
    fi
}

# Boot the built engine against a throwaway copy of this workspace's database
# (`lucidos-engine preflight`), so a binary that cannot boot is never published.
# Every later spawn, including a crash respawn, runs the published binary, so
# this is the only place a failure is cheap. Skipped when the workspace has no
# database. Skipped with a WARNING when Postgres does not answer, since the
# binary is then unchecked.
preflight_built_engine() {
    local engine="$1" ports_file="${WORKSPACE:-}/.lucidos/ports" port db found probe=0
    if [ -z "${WORKSPACE:-}" ] || [ ! -f "$ports_file" ]; then
        echo "Preflight skipped: no database is recorded for this workspace."
        return 0
    fi
    port="$(sed -n 's/^PG_PORT=//p' "$ports_file" | head -n 1)"
    db="$(sed -n 's/^PG_DATABASE=//p' "$ports_file" | head -n 1)"
    if [ -z "$port" ] || [ -z "$db" ]; then
        echo "Preflight skipped: no database is recorded for this workspace."
        return 0
    fi
    found="$(shared_pg_psql postgres -tAc \
        "SELECT 1 FROM pg_database WHERE datname=$(_shared_pg_literal "$db")" 2>/dev/null)" || probe=$?
    if [ "$probe" -ne 0 ]; then
        echo "WARNING: Preflight skipped: Postgres did not answer, so this engine is unchecked against $db."
        return 0
    fi
    if [ "$found" != 1 ]; then
        echo "Preflight skipped: the workspace database $db does not exist yet."
        return 0
    fi
    echo "Preflight: booting the built engine on a copy of $db..."
    # The URL carries the password, so it goes only into the child's environment.
    local output status=0
    output="$(DATABASE_URL="postgres://$PG_USER:$(shared_pg_password)@localhost:$port/$db" \
        LUCIDOS_WORKSPACE="$WORKSPACE" "$engine" preflight 2>&1)" || status=$?
    printf '%s\n' "$output"
    if [ "$status" -ne 0 ]; then
        # A line starting `ERROR:` is what the engine's build-failure toast shows.
        echo "ERROR: the built engine cannot boot on this workspace's data: $(
            printf '%s\n' "$output" | sed -n 's/^Error: //p' | tail -n 1)"
        echo "The previous engine binary stays published, so nothing restarts onto this build."
        return 1
    fi
}

# Publish the launch binaries only when the built engine passes the preflight,
# and only if the preflight tested the binary that gets published. The uplift
# path is shared, so another build can replace the binary while a copy of a big
# database restores.
publish_after_preflight() {
    local uplift_dir="$1" launch_dir="$2" with_window="$3"
    local engine="$uplift_dir/lucidos-engine" tested
    tested="$(cksum < "$engine" 2>/dev/null)"
    preflight_built_engine "$engine" || return 1
    if [ "$(cksum < "$engine" 2>/dev/null)" != "$tested" ]; then
        echo "ERROR: the built engine changed while the preflight ran, so it was not published. Build again."
        return 1
    fi
    publish_all_launch_binaries "$uplift_dir" "$launch_dir" "$with_window"
}

# Locate already-built binaries for the no-build path. Prefers the published
# launch dir; falls back to cargo's shared uplift path with a warning (a fresh
# checkout, or a hand-run `cargo build`, has no published dir yet). Profile
# order mirrors the historical behavior: a release or e2e request falls back to
# debug, a debug request reaches for nothing else. Sets ENGINE_BIN + GATEWAY_BIN as a
# pair so they can never come from different builds; returns non-zero when
# nothing is on disk.
locate_launch_binaries() {
    local variant
    variant="$(engine_build_variant_slug)"
    # `dirs` and `published` are parallel: entry i of `published` is "1" when
    # dirs[i] is a launch dir this script publishes to, "" when it is cargo's
    # shared uplift path. Tracked explicitly rather than pattern-matched out of
    # the path, because a glob over the directory NAME has to be kept in step
    # with `launch_bin_dir` by hand and silently stops matching when that moves.
    # It did: the discriminator was `*/launch/*`, which no longer matches now
    # that the published dir is `.launch/`, so every published launch would have
    # printed the "no published engine binary yet" warning.
    local -a dirs=() published=()
    local profile
    profile="$(engine_build_profile)"
    if [ "$profile" != debug ]; then
        dirs+=("$(launch_bin_dir "$profile" "$variant")" "$PROJECT_DIR/target/$profile")
        published+=("1" "")
    fi
    dirs+=("$(launch_bin_dir debug "$variant")" "$PROJECT_DIR/target/debug")
    published+=("1" "")

    local i dir
    for i in "${!dirs[@]}"; do
        dir="${dirs[$i]}"
        # BOTH must be there. A dir holding only the engine is a half-finished
        # build (its gateway publish failed, or someone ran `cargo build -p
        # lucidos-engine` alone); selecting it on the engine alone would pair a
        # fresh engine with a missing or older gateway, and the launch would die
        # later with a much less obvious error than "run with -b".
        { [ -f "$dir/lucidos-engine" ] && [ -f "$dir/lucidos-gateway" ]; } || continue
        ENGINE_BIN="$dir/lucidos-engine"
        GATEWAY_BIN="$dir/lucidos-gateway"
        if [ -z "${published[$i]}" ]; then
            echo "WARNING: no published engine binary yet. Launching from cargo's shared"
            echo "         uplift path $dir, which every build variant in this checkout"
            echo "         writes to. Run with -b to build and publish a pinned one."
        fi
        return 0
    done

    echo "No engine binary found. Run with -b to build."
    return 1
}

# ── build_or_find_engine ────────────────────────────────────────────────
# Build engine (+ gateway + cli) if BUILD is set, otherwise find existing
# binaries. Sets ENGINE_BIN and GATEWAY_BIN to the PUBLISHED launch binaries
# (see "Published launch binaries" above).
build_or_find_engine() {
    if [ -z "${BUILD:-}" ]; then
        locate_launch_binaries || exit 1
        return 0
    fi

    local launch_dir uplift_dir with_window=""
    launch_dir="$(launch_bin_dir)"
    uplift_dir="$PROJECT_DIR/target/$(engine_build_profile)"
    if [ -f "$(dev_window_bin "$launch_dir")" ]; then
        with_window=1
    fi

    # The build waits on cargo's own lock for any cargo holding target/. Never
    # signal it or delete its lock file: it may be another worktree's build
    # (ADR 0294).
    echo ""
    echo "Building engine..."
    # A caller in an `||` or `if` runs with errexit off, so a failed compile
    # must fail here, or the uplift fallback below serves a stale binary.
    run_launch_cargo_builds "$with_window" || return 1
    prune_stale_debug_objects "$uplift_dir/deps"
    publish_after_preflight "$uplift_dir" "$launch_dir" "$with_window" || return 1

    # Did this build actually produce a binary for the source on disk now? If
    # HEAD moved while cargo was running, it didn't — rebuild ONCE against the
    # source that is there now. Bounded at one retry, and a surviving mismatch
    # only WARNS: the compile genuinely succeeded, the engine's direction guard
    # decides whether to offer the binary, and failing here would surface a
    # false "New engine version failed to build" and abort the Apply-triggered
    # background rebuild for every co-located workspace.
    local state
    state="$(published_build_state "$launch_dir/lucidos-engine")"
    if [ "$state" = "stale" ]; then
        echo "Built engine is not the source now on disk (HEAD moved during the build,"
        echo "or another build clobbered the shared uplift path) — rebuilding once..."
        run_launch_cargo_builds "$with_window" || return 1
        prune_stale_debug_objects "$uplift_dir/deps"
        publish_after_preflight "$uplift_dir" "$launch_dir" "$with_window" || return 1
        state="$(published_build_state "$launch_dir/lucidos-engine")"
    fi
    if [ "$state" = "stale" ]; then
        echo "WARNING: published engine is build id" \
            "$("$launch_dir/lucidos-engine" --build-id 2>/dev/null || echo '?')" \
            "while HEAD is $(git -C "$PROJECT_DIR" rev-parse --short HEAD 2>/dev/null || echo '?')."
        echo "         Something keeps rebuilding an earlier checkout state. The running"
        echo "         engine will refuse to offer it as a new version; rebuild with -b."
    fi


    # The published pair is already signed with the stable dev identity — that
    # happens inside publish_launch_binaries, on the temp file, BEFORE the
    # rename, so no peer can catch a binary mid-`codesign`. Only the fallback
    # needs signing here, because it launches cargo's uplift path directly.
    if [ -x "$launch_dir/lucidos-engine" ] && [ -x "$launch_dir/lucidos-gateway" ]; then
        ENGINE_BIN="$launch_dir/lucidos-engine"
        GATEWAY_BIN="$launch_dir/lucidos-gateway"
    else
        echo "WARNING: could not publish the launch binaries to $launch_dir —"
        echo "         falling back to cargo's shared uplift path $uplift_dir."
        ENGINE_BIN="$uplift_dir/lucidos-engine"
        GATEWAY_BIN="$uplift_dir/lucidos-gateway"
        # Best-effort; no-op off macOS or until ./scripts/dev-codesign-setup.sh
        # has been run once. The Designated Requirement is identifier +
        # certificate leaf — no CDHash, no path — so macOS TCC grants persist
        # across rebuilds and across the published/uplift path split.
        sign_engine_binary "$ENGINE_BIN"
        sign_engine_binary "$GATEWAY_BIN"
    fi
}

# ── swap_ports ──────────────────────────────────────────────────────────
# Dev runtime topology (ADR 0014 §4 — "one engine serves both gateway-fronted
# and legacy-direct access, per request"):
#   • ENGINE_PORT  = VITE_PORT (5173+offset), the port the engine binds. Under
#     the gateway it is LOOPBACK-ONLY, so it answers on this machine alone and
#     the gateway is the only network door (ADR 0094). Legacy direct-engine mode
#     binds it network-wide, because there the engine IS the front. This stays
#     PER-WORKSPACE so multiple engines coexist.
#   • GATEWAY_PORT = a FIXED machine-global port (DEFAULT_DEV_GATEWAY_PORT;
#     override with LUCIDOS_DEV_GATEWAY_PORT). The packaged desktop app keeps
#     its own port, so dev + packaged coexist out of the box. NOT the
#     per-workspace API_PORT. There is ONE shared gateway per machine; it binds
#     this fixed port and serves `/<slug>/` (proxying to each engine) + the
#     picker at `/~/`. A fixed port is required because every workspace launch
#     reuses the SAME gateway (ADR 0014 §10) rather than starting its own.
# Both are reachable at once in dev; the engine serves the built dist/ directly
# via LUCIDOS_STATIC_DIR (no Vite in the serving path). Writes .lucidos/ports
# (the engine's direct port, so the CLI reaches the engine). Exports env vars.
swap_ports() {
    ENGINE_PORT="$VITE_PORT"
    GATEWAY_PORT="${LUCIDOS_DEV_GATEWAY_PORT:-$DEFAULT_DEV_GATEWAY_PORT}"

    # The ports file records the engine's direct port — the CLI / cross-workspace
    # callers talk to the engine, not the gateway.
    #
    # Key-preserving, so the PROTO line detect_tls wrote survives. It used to
    # truncate and write PROTO back afterwards, which held only while both ran
    # in that order. Every reader falls back to https when the line is missing.
    # On a machine with no dev certs the engine then serves plain http, and each
    # caller fails at the TLS handshake with "record overflow".
    ports_file_set "$WORKSPACE/.lucidos/ports" \
        "API_PORT=$ENGINE_PORT" \
        "VITE_PORT=$ENGINE_PORT" \
        "PG_PORT=$PG_PORT" \
        "PG_DATABASE=$(workspace_database_name)"

    # LUCIDOS_API_PORT here is the ENGINE's port (the legacy/direct + tauri/e2e
    # paths spawn the engine on it). start_gateway overrides LUCIDOS_API_PORT to
    # GATEWAY_PORT for the gateway process itself.
    export LUCIDOS_API_PORT="$ENGINE_PORT"
    DATABASE_URL="$(workspace_database_url)"
    export DATABASE_URL
    export WORKSPACE_PATH="$WORKSPACE"
    # The engine (direct) and the gateway (picker) both serve the built dist/.
    # Guard at the choke point: every launch path funnels through here, so this
    # is where a worktree-pinned dist/ must be refused (the entry scripts also
    # assert earlier, for a cleaner failure before any work is done).
    assert_stack_not_worktree_pinned "$PROJECT_DIR" || exit 1
    # shellcheck disable=SC2153 # FRONTEND_DIR is a required input set by the sourcing script (see header)
    export LUCIDOS_STATIC_DIR="$FRONTEND_DIR/dist"
}

source "$(dirname "${BASH_SOURCE[0]}")/sleep.sh"
source "$(dirname "${BASH_SOURCE[0]}")/engine_supervisor.sh"
# Gateway gets its OWN supervisor (decoupled from the engine's) — a machine-global
# daemon that survives the launching shell / terminal (run_gateway_supervised).
source "$(dirname "${BASH_SOURCE[0]}")/gateway_supervisor.sh"
source "$(dirname "${BASH_SOURCE[0]}")/codesign.sh"

# ── enable_clamshell_prevention ────────────────────────────────────────
enable_clamshell_prevention() {
    [ "$(uname)" = "Darwin" ] || return 0   # macOS-only (pmset/clamshell); no-op on Linux/CI
    mkdir -p "$SLEEP_LOCK_DIR"
    cleanup_stale_sleep_locks

    local ws_hash
    ws_hash="$(hash_string "$WORKSPACE")"
    echo "$$" > "$SLEEP_LOCK_DIR/$ws_hash"

    ensure_sudoers_pmset

    if sudo -n pmset disablesleep 1 2>/dev/null; then
        return 0
    fi
    echo "WARNING: Could not disable clamshell sleep. Lid close will sleep the Mac."
    echo "  Fix: start any workspace from a terminal once to set up passwordless pmset."
}

# ── Network bind (dev) ───────────────────────────────────────────────────
# The engine and the gateway both default to loopback. Only the GATEWAY opts
# back into the network here: it authenticates every caller (ADR 0094), so it is
# the one process that may face one. A directly-launched engine authenticates
# nobody, so it stays where the engine's own resolver puts it.
# ~/.lucidos/network.toml owns the address when it exists. The shell only
# chooses between pinning a bind and deferring to that resolver.
network_toml_exists() {
    [ -f "$(lucidos_home_dir)/network.toml" ]
}

# Bind for a directly-launched engine (start_engine): legacy no-gateway dev
# (tauri-dev.sh included, under LUCIDOS_NO_GATEWAY) and e2e. Loopback is what
# makes this port safe to serve
# unauthenticated, so widening it is a deliberate act by the developer, never a
# launch-script default. Widened, the engine asks every caller for the local
# token (ADR 0155), which a browser cannot present.
#
# e2e pins loopback instead of deferring, because both suites address the engine
# as `localhost` and must not inherit a developer's tailnet bind.
# LUCIDOS_BIND_ADDR rather than LUCIDOS_BIND_LOOPBACK, because the latter
# doubles as the `behind_gateway` signal. That signal moves the API base URL
# handed to subprocesses (api/actor.rs) and suppresses the lucidos.toml port pin
# (engine_impl/construction.rs). LUCIDOS_BIND_ADDR outranks LUCIDOS_BIND_ALL and
# network.toml alike (crates/lucidos-engine/src/net_config.rs), and carries no
# second meaning.
apply_dev_engine_bind() {
    if [ "${SCRIPT_NAME:-}" = "e2e" ]; then
        export LUCIDOS_BIND_ADDR=127.0.0.1
        return 0
    fi
    # Set nothing. The engine resolves its own bind: an explicit LUCIDOS_BIND_*
    # from the developer's shell, else ~/.lucidos/network.toml, else loopback.
    return 0
}

# Bind for the dev gateway (start_gateway). Same rule, gateway-scoped var.
apply_dev_gateway_bind() {
    if network_toml_exists; then
        unset LUCIDOS_GATEWAY_BIND_ALL  # gateway resolver reads network.toml
    else
        export LUCIDOS_GATEWAY_BIND_ALL=1
    fi
}

# The scheme a live engine answers /api/v1/health on, or empty if none does.
#
# https first, then http, mirroring gateway_stop_status. Probing `$PROTO` alone
# is what broke tauri-dev: the gateway strips LUCIDOS_TLS_CERT from every engine
# it spawns (crates/lucidos-gateway/src/stack.rs), so on a machine with dev certs
# the live engine serves plain http while detect_tls picked https. The probe then
# read a healthy engine as dead and the caller spawned a second one onto the held
# port, which died with AddrInUse.
#
# Exits 0 even when neither scheme answers. That is an ordinary answer, and the
# caller assigns this into a variable under `set -e`, where a non-zero command
# substitution would take the whole launch down. An `if` with no `else` already
# yields 0; the trailing `return 0` states the contract so a later restructure
# into an `&&` chain cannot quietly drop it.
#
# Both probes take probe_engine_on_port's time budget, because every gateway
# launch runs this one too.
engine_health_scheme() { # <port>
    local port="$1" budget="${LUCIDOS_HEALTH_PROBE_TIMEOUT_S:-2}"
    if curl -sk --max-time "$budget" "https://localhost:$port/api/v1/health" >/dev/null 2>&1; then
        printf 'https'
    elif curl -s --max-time "$budget" "http://localhost:$port/api/v1/health" >/dev/null 2>&1; then
        printf 'http'
    fi
    return 0
}

# Take the scheme a reused engine actually serves. Every URL this launch goes on
# to build (the banner, the desktop window, the ports file every CLI caller
# reads) must name a door that answers, and the running engine is the authority
# on its own.
adopt_engine_scheme() { # <scheme>
    if [ "$1" = "$PROTO" ]; then
        return 0
    fi
    echo "Engine on port $ENGINE_PORT serves $1, not $PROTO. Adopting $1."
    PROTO="$1"
    ports_file_set "$WORKSPACE/.lucidos/ports" "PROTO=$PROTO"
}

# The gateway-mode twin of adopt_engine_scheme. The ports file's PROTO names the
# engine on API_PORT, which every CLI caller dials. Here $PROTO names the
# gateway instead: the banner, the window and gateway_curl build on it. The
# gateway keeps its TLS and strips it from the engines it spawns (ADR 0096).
#
# So only the file follows the engine. A reused gateway and engine run neither
# adopt_engine_scheme nor the gateway's own ports publish, so nothing else
# corrects the scheme detect_tls derived from .certs/. No answer is no evidence,
# and then the file keeps what it has.
record_engine_scheme() {
    local live_scheme
    live_scheme="$(engine_health_scheme "$ENGINE_PORT")"
    if [ -z "$live_scheme" ] || [ "$live_scheme" = "$PROTO" ]; then
        return 0
    fi
    echo "Engine on port $ENGINE_PORT serves $live_scheme. Recording it in the ports file."
    ports_file_set "$WORKSPACE/.lucidos/ports" "PROTO=$live_scheme"
}

# How long start_engine lets a dying engine release the port before it calls the
# port lost. The engine's own graceful-shutdown budget (main.rs).
ENGINE_DRAIN_WAIT_S=10

# ── start_engine ────────────────────────────────────────────────────────
# The pid of the engine that owns this workspace, when this launch's supervisor
# started it. An engine from any earlier launch prints nothing.
engine_pid_of_this_launch() {
    local pid parent
    pid="$(engine_live_pid "$WORKSPACE")"
    [ -n "$pid" ] || return 0
    parent="$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')"
    [ "$parent" = "$ENGINE_SUPERVISOR_PID" ] && echo "$pid"
    return 0
}

# Run engine in background and wait for health. The engine holds
# the Mac awake itself while work runs (ADR 0366), so no caffeinate here.
# Reuses an existing healthy engine if one is already running for this workspace.
# Sets ENGINE_PID.
start_engine() {
    # Direct-front launch (legacy no-gateway dev, e2e): the engine IS
    # the user-facing door on its own port. On loopback it serves every local
    # caller without a credential, so it stays there unless the developer widens
    # it deliberately. The gateway path does NOT use this function. It spawns
    # engines itself with the right bind (see gateway stack.rs spawn_engine).
    apply_dev_engine_bind

    # Adopt an engine already healthy on our port instead of spawning a second
    # one. The scheme is probed, never assumed (see engine_health_scheme).
    local live_scheme existing_pid
    live_scheme="$(engine_health_scheme "$ENGINE_PORT")"
    if [ -n "$live_scheme" ]; then
        existing_pid="$(engine_live_pid "$WORKSPACE")"
        if [ -n "$existing_pid" ]; then
            adopt_engine_scheme "$live_scheme"
            echo ""
            echo "Reusing existing engine (PID $existing_pid) on port $ENGINE_PORT"
            ENGINE_PID="$existing_pid"
            enable_clamshell_prevention
            return
        fi
    fi

    # Nothing to adopt, so the port has to come free or the spawn below cannot
    # bind. The supervisor would respawn it into a 90 second wall of dots ending
    # in a timeout that names no cause, so name the holder instead.
    #
    # Poll rather than sample once. Without `-b`, kill_stale_processes signals
    # the previous dev script's engine and never waits for it, so an
    # instantaneous check races a drain that the engine gets 10s to finish
    # (main.rs). ENGINE_DRAIN_WAIT_S matches that budget: anything still holding
    # the port afterwards is not ours to wait for.
    if ! _await_engine_port_released "$ENGINE_PORT" "" "$ENGINE_DRAIN_WAIT_S"; then
        local holder holder_cmd held_scheme
        holder="$(lsof -ti :"$ENGINE_PORT" -sTCP:LISTEN 2>/dev/null | head -1 || true)"
        holder_cmd="$(ps -p "$holder" -o comm= 2>/dev/null || true)"
        # Re-probe rather than reuse live_scheme. This branch is also reached
        # when the holder IS healthy and merely unclaimable, because no engine
        # owns this workspace, and a message that says nothing answers would
        # send the reader hunting the wrong fault.
        held_scheme="$(engine_health_scheme "$ENGINE_PORT")"
        echo ""
        echo "ERROR: port $ENGINE_PORT is held by ${holder_cmd:-an unknown process} (PID ${holder:-unknown})"
        if [ -n "$held_scheme" ]; then
            echo "       It is healthy over $held_scheme, but no engine holds"
            echo "       $(engine_lock_path "$WORKSPACE"), so this launch cannot adopt it."
        else
            echo "       and nothing answers /api/v1/health there on http or https."
        fi
        echo ""
        echo "  In the normal dev topology the gateway owns this port. Launch"
        echo "  through it instead of starting a direct engine:"
        echo "    ./scripts/web-dev.sh -w $WORKSPACE"
        echo "  Or stop whatever holds the port first:"
        echo "    ./scripts/stop.sh -w $WORKSPACE"
        exit 1
    fi

    echo ""
    echo "Starting Lucidos engine..."
    # Rotate log if > 10 MB
    if [ -f "$ENGINE_LOG" ]; then
        local log_size
        log_size=$(stat -f %z "$ENGINE_LOG" 2>/dev/null || echo 0)
        if [ "$log_size" -gt 10485760 ]; then
            tail -c 1048576 "$ENGINE_LOG" > "$ENGINE_LOG.tmp" 2>/dev/null && mv "$ENGINE_LOG.tmp" "$ENGINE_LOG"
            echo "[$SCRIPT_NAME] Log rotated (was $(( log_size / 1048576 )) MB)" >> "$ENGINE_LOG"
        fi
    fi

    # Raise FD limit — macOS defaults to 256 which is too low for the engine
    ulimit -n 8192 2>/dev/null

    enable_clamshell_prevention

    # Spawn the supervisor (engine_supervisor.sh:run_supervised) as a
    # backgrounded subshell. It loops the engine binary so an unexpected
    # kill (SIGKILL from a stale worktree's ports.sh, OOM, panic) becomes
    # a 1–30 s blip instead of a session outage. Legit stops (exit 0, or
    # the default exit of SIGINT or SIGUSR1, or SIGTERM to the supervisor
    # itself from kill_stale_processes' `pkill -P`) flow through cleanly.
    ( run_supervised "$WORKSPACE/.lucidos" "$ENGINE_LOG" "$ENGINE_BIN" ) &
    ENGINE_SUPERVISOR_PID=$!

    # Wait for engine to be ready. Cold boot does pgvector init, migrations and
    # the memory index load, which can push past 30s on a fresh workspace or a
    # slow disk. Give it 90s before declaring failure. (The embedding model is
    # NOT in that window: it loads in the background and boot never waits on
    # it.)
    #
    # Ready means the engine of THIS launch owns the workspace and answers. The
    # engine publishes engine.pid itself once it holds the owner lock, so until
    # then the file may still name a predecessor that is draining (ADR 0438).
    # Polls five times a second, so a boot of a second or two is not rounded
    # up to the next whole second. Prints one dot a second.
    echo -n "Waiting for engine"
    local engine_ready="" tick
    for tick in $(seq 1 450); do
        if ! kill -0 "$ENGINE_SUPERVISOR_PID" 2>/dev/null; then
            break
        fi
        ENGINE_PID="$(engine_pid_of_this_launch)"
        if [ -n "$ENGINE_PID" ] \
            && curl -sk "$PROTO://localhost:$ENGINE_PORT/api/v1/health" >/dev/null 2>&1; then
            echo " ready!"
            engine_ready="yes"
            break
        fi
        [ $((tick % 5)) -ne 0 ] || echo -n "."
        sleep 0.2
    done

    if [ -z "$engine_ready" ]; then
        echo ""
        echo "ERROR: Engine failed to start within 90 seconds. Check logs:"
        tail -20 "$ENGINE_LOG"
        kill -KILL "$ENGINE_SUPERVISOR_PID" 2>/dev/null || true
        exit 1
    fi
    # The engine's own account of where its boot went.
    grep -E "\[pid:$ENGINE_PID\].*(Migrations checked|Boot stages)" "$ENGINE_LOG" 2>/dev/null | tail -n 2 \
        | sed -E 's/^.*\[Startup\] /Engine: /' || true
}

# ── Workspace gateway (ADR 0014) ──────────────────────────────────────────
# Dev runs ONE machine-global `lucidos-gateway` binary as the user-facing front
# (on DEFAULT_DEV_GATEWAY_PORT; the packaged app keeps its own port).
# It reverse-proxies /<slug>/ to each
# workspace's engine (which it spawns + supervises) and serves the workspace
# picker behind the sigil namespace /~/. The registry, pidfile, and log live
# under a SHARED, machine-global dir ($HOME/.lucidos/gateway) — NOT per-workspace
# — so every `web-dev.sh` launch accumulates into ONE registry served by ONE
# gateway, and the picker lists every workspace ever launched (ADR 0014 §10). A
# workspace's engine binds its own per-workspace port (ENGINE_PORT = VITE_PORT)
# on LOOPBACK, so only this machine reaches it and every other device goes
# through the gateway.
# More workspaces are also created from the picker (the gateway then provisions
# their Docker Postgres itself). Set LUCIDOS_NO_GATEWAY=1 to fall back to the
# legacy direct-engine model (engine serves the app at / directly).

# The gateway's app-data dir is machine-global (one gateway per machine), so it
# is keyed off $HOME, not $WORKSPACE. `gateway_data_dir` lives in
# storage_root.sh, beside the home it defaults under.
gateway_pidfile()  { echo "$(gateway_data_dir)/gateway.pid"; }
gateway_log()      { echo "$(gateway_data_dir)/gateway.log"; }

# `curl` carrying the machine-local credential the gateway's control plane
# requires. Mirrors `lucidos-local-token`: same header, same path.
#
# A loopback address proves nothing to the gateway, because `tailscale serve`
# proxies remote requests from `127.0.0.1` too. Reading this mode 0600 file is
# what a remote caller cannot do.
#
# A missing file is normal and falls through to a plain `curl`: a workspace can
# run with no gateway at all, and every caller here already tolerates failure.
gateway_curl() {
    local token_file
    token_file="$(lucidos_home_dir)/local-token"
    if [ -r "$token_file" ]; then
        curl -H "x-lucidos-local-token: $(cat "$token_file")" "$@"
    else
        curl "$@"
    fi
}

# ── which gateway owns a workspace ──────────────────────────────────────
# More than one gateway runs on this machine: the dev one, serving
# source-checkout workspaces, and the packaged Lucidos.app one. Each has
# its own registry, and a stop posted to the wrong one reaches a process that
# has never heard of the workspace. So the port cannot be a constant.
#
# It does not have to be. The gateway puts its own port into every engine it
# spawns (`engine_env_overrides` in crates/lucidos-gateway/src/stack.rs), so the
# engine being stopped carries the answer in its own environment.

# The gateway port of the engine that owns workspace $1, or nothing.
#
# Nothing means: no engine holds the workspace's lock, an engine started with no
# gateway (LUCIDOS_NO_GATEWAY dev, the e2e harness), or a platform whose `ps`
# refuses `-E`. Only macOS prints another process's environment that way, and
# Linux lands on the same fallback the whole script used before. The caller
# falls back and says so, rather than guessing which gateway an engine belonged
# to.
engine_gateway_port() {
    local pid
    pid="$(engine_live_pid "$1")"
    [ -n "$pid" ] || return 0
    proc_env_value "$(ps -E -p "$pid" -o command= 2>/dev/null)" LUCIDOS_GATEWAY_PORT
}

# POST the gateway's stop control for $2 on port $1, printing the HTTP status.
#
# https first, then http: the dev gateway serves TLS when certs exist and plain
# http otherwise. A refused or unencrypted connection gives curl's own `000`,
# which is the signal to try the other scheme rather than a gateway answer.
# "000" is also what a caller sees when no gateway is listening at all.
#
# Each call is `|| true`, and that is load-bearing under the caller's `set -e`:
# curl exits non-zero on a refused connection, which would otherwise take
# stop.sh down before it ever reached the engine signal. The status still
# reaches us, because `-w` prints 000 on its way out.
gateway_stop_status() {
    local port="$1" slug="$2" path code
    path="/~/api/v1/control/workspaces/$slug/stop"
    code="$(gateway_curl -sk -o /dev/null -w '%{http_code}' -X POST "https://localhost:$port$path" 2>/dev/null || true)"
    case "${code:-000}" in
        000) code="$(gateway_curl -s -o /dev/null -w '%{http_code}' -X POST "http://localhost:$port$path" 2>/dev/null || true)" ;;
    esac
    printf '%s' "${code:-000}"
}

# Stable, filesystem/URL-safe slug from the workspace dir basename — the routing
# key (/<slug>/). Mirrors gateway::registry::slugify; empty → "workspace".
workspace_slug() {
    local s
    s="$(basename "$WORKSPACE" | tr '[:upper:]' '[:lower:]' | sed -E 's/[^a-z0-9]+/-/g; s/^-+//; s/-+$//')"
    [ -n "$s" ] && echo "$s" || echo "workspace"
}

# The URL a dev window loads (tauri-dev.sh passes it as LUCIDOS_DEV_WINDOW_URL).
#
# Gateway mode reaches the workspace through the gateway at /<slug>/, which is
# the door the packaged app navigates to (desktop.rs). That prefix is what
# makes the engine stamp `<base href="/<slug>/">`, so a dev window exercises
# the same routing the packaged one does. Legacy direct-engine mode
# (LUCIDOS_NO_GATEWAY) has no gateway, so the engine root is the front.
#
# The condition is show_banner's, deliberately: the banner and the window must
# never name different URLs.
desktop_window_url() { # <gateway_mode> <proto> <gateway_port> <ws_id> <engine_port>
    local mode="$1" proto="$2" gateway_port="$3" ws_id="$4" engine_port="$5"
    if [ -n "$mode" ] && [ -n "$ws_id" ]; then
        printf '%s://localhost:%s/%s/' "$proto" "$gateway_port" "$ws_id"
    else
        printf '%s://localhost:%s' "$proto" "$engine_port"
    fi
}

# Register this workspace with the gateway at `base` (default: the dev
# gateway on GATEWAY_PORT), and set GATEWAY_WS_ID from its answer.
#
# Through the adopt endpoint, never by writing workspaces.json: the gateway owns
# that file and writes it under its own lock (ADR 0438). The port is the
# engine's own (ENGINE_PORT = VITE_PORT, ADR 0014 §4). Name and autostart are
# left out, so a re-adopt keeps what the picker set, and a new entry is named
# after its directory with autostart off.
#
# A refusal fails the launch and quotes the gateway. Carrying on would leave a
# window on a route the gateway does not know.
adopt_into_gateway() { # [base-url]
    local base="${1:-$PROTO://localhost:$GATEWAY_PORT}" body response code
    body="$(python3 -c 'import json, sys; print(json.dumps({"dir": sys.argv[1], "port": int(sys.argv[2])}))' \
        "$WORKSPACE" "$ENGINE_PORT")"
    response="$(gateway_curl -sk --max-time 10 -w '\n%{http_code}' -X POST \
        -H 'content-type: application/json' --data "$body" \
        "$base/~/api/v1/control/workspaces/adopt" 2>/dev/null || true)"
    code="${response##*$'\n'}"
    response="${response%$'\n'*}"
    if [ "$code" != 200 ]; then
        echo "ERROR: the gateway at $base did not register $WORKSPACE (HTTP ${code:-no answer})." >&2
        [ -z "$response" ] || echo "       It said: $response" >&2
        return 1
    fi
    GATEWAY_WS_ID="$(printf '%s' "$response" \
        | python3 -c 'import json, sys; print(json.load(sys.stdin)["workspace"]["id"])')" || return 1
    echo "Gateway: workspace '$GATEWAY_WS_ID' → engine :$ENGINE_PORT, database $(workspace_database_name), gateway at $base"
}

# Wait for /<slug>/api/v1/health — the engine the gateway spawned.
#
# `-f`, not a bare `-s`: curl exits 0 for ANY http response, and the gateway
# answers an unrouted slug with a 404 or a 503 rather than by hanging up. So
# without it this loop printed "ready!" on its first tick whether or not the
# workspace could be reached, which is the whole thing it is here to find out.
#
# `gateway_curl`, because the gateway gates every workspace path, health
# included. A bare curl gets 401 on a healthy engine and waits out all 90 ticks.
wait_for_workspace_health() {
    echo -n "Waiting for workspace '$GATEWAY_WS_ID' engine"
    for _ in $(seq 1 90); do
        if gateway_curl -fsk "$PROTO://localhost:$GATEWAY_PORT/$GATEWAY_WS_ID/api/v1/health" >/dev/null 2>&1; then
            echo " ready!"; return 0
        fi
        echo -n "."; sleep 1
    done
    echo ""
    echo "WARNING: workspace engine not healthy yet — check the picker or the gateway log:"
    echo "  $(gateway_log)"
}

# ── workspace_engine_restart_is_needed ──────────────────────────────────
# Whether this launch must ask the gateway to RESTART this workspace's engine,
# rather than adopt the one already answering.
#
# Pure, so the decision is testable without a gateway. Args: $1 = ENGINE_ONLY,
# $2 = non-empty when our engine is already serving its port. The launch has
# just registered the workspace, so the gateway's supervise tick adopts it.
#
# A restart is the wrong ask when the engine is already up. It tears down every
# in-flight thread, and those settle crash-shaped because a script carries no
# device (RestartIntentNotify::Skipped, crates/lucidos-gateway/src/stack.rs).
# start_engine has always adopted on the direct path, and this is the same rule
# for the gateway one. See
# docs/plans/2026-09-18-a-launch-never-tears-down-a-healthy-engine.md.
#
# Two launches still need the POST. --engine-only IS the Apply switch, whose
# whole job is to respawn onto the freshly built binary. A `-b` arrives here with
# its engine already reaped by kill_stale_processes, so the probe says "not
# running" and the POST becomes a start, which is what it is for: a new workspace
# defaults to autostart off, so the gateway's own boot does not spawn it.
workspace_engine_restart_is_needed() { # <engine_only> <engine_is_live>
    [ -n "$1" ] && return 0
    [ -z "$2" ] && return 0
    return 1
}

# Make sure this workspace's engine is running, and reachable at /<slug>/.
#
# The health wait runs on BOTH branches. An adopted engine may have no gateway
# route yet, and `adopt_running_engines` installs one on the next supervise tick.
# The wait cannot hurry that along: the gateway lazy-starts on a document
# navigation only, never on the API call this makes.
#
# Both branches end on record_engine_scheme. Adopting skips the gateway's spawn,
# and with it the gateway's ports publish. The restart POST is fire-and-forget,
# so its branch cannot count on that publish either.
ensure_workspace_engine_running() {
    local live=""
    if engine_on_port_serves_workspace "$ENGINE_PORT" "$WORKSPACE"; then
        live=1
    fi
    if workspace_engine_restart_is_needed "${ENGINE_ONLY:-}" "$live"; then
        gateway_curl -sk -X POST "$PROTO://localhost:$GATEWAY_PORT/~/api/v1/control/workspaces/$GATEWAY_WS_ID/restart" >/dev/null 2>&1 || true
    else
        echo "Reusing the running engine for '$GATEWAY_WS_ID' on port $ENGINE_PORT"
    fi
    wait_for_workspace_health
    record_engine_scheme
}

# Start (or reuse) the ONE shared workspace gateway on the fixed GATEWAY_PORT. It
# spawns + supervises each workspace's engine network-bound on its own
# ENGINE_PORT (so the app is reachable directly there too, ADR 0014 §4) and
# routes /<slug>/ + the picker at /~/. Once the gateway is up, this workspace is
# registered through its adopt endpoint. New workspaces default to autostart
# OFF, so the gateway's own boot does NOT spawn this one, and
# ensure_workspace_engine_running starts (or respawns, for an Apply) it. Sets GATEWAY_PID; leaves ENGINE_SUPERVISOR_PID
# EMPTY (the detached gateway supervisor is a machine-global daemon web-dev.sh
# must never `wait` on — its non-tty wait falls back to the pidfile poll).
start_gateway() {
    GATEWAY_MODE=1
    # GATEWAY_PORT (fixed, see swap_ports) and ENGINE_PORT (=VITE_PORT) are set by
    # swap_ports. The gateway's data dir / pidfile / log are machine-global.
    local gw_pidfile gw_log
    ensure_storage_root "$(lucidos_home_dir)"
    ensure_storage_root "$(gateway_data_dir)"
    gw_pidfile="$(gateway_pidfile)"
    gw_log="$(gateway_log)"

    export LUCIDOS_API_PORT="$GATEWAY_PORT"
    LUCIDOS_GATEWAY_DATA="$(gateway_data_dir)"
    export LUCIDOS_GATEWAY_DATA
    export LUCIDOS_GATEWAY_PG_BACKEND="docker"
    export LUCIDOS_GATEWAY_PG_PORT="$PG_PORT"
    LUCIDOS_GATEWAY_PG_CONTAINER="$(shared_pg_container)"
    export LUCIDOS_GATEWAY_PG_CONTAINER
    export LUCIDOS_ENGINE_BIN="$ENGINE_BIN"
    # Nothing sets LUCIDOS_GATEWAY_ENGINE_LOOPBACK here, so the gateway spawns
    # loopback-only engines in dev exactly as it does packaged. That is the
    # point: the gateway authenticates every network caller (ADR 0094), and a
    # network-bound engine port is a way past it. Reaching a workspace from
    # another device goes through the gateway now, at
    # `https://<host>:$GATEWAY_PORT/<slug>/`, and pairs. The variable is still
    # read, so a deployment that needs the old topology can set it to 0.
    #
    # The gateway itself binds all interfaces in dev, which is now the only
    # network door. It defaults to loopback-only (the packaged security posture;
    # "deployments that intentionally front Lucidos on the network must opt in
    # explicitly", crates/lucidos-gateway/src/server.rs). Dev IS such a
    # deployment: the user reaches the picker and `/<slug>/` routing from other
    # devices, e.g. an iOS PWA over Tailscale. Without this opt-in a gateway
    # rebuild+reload comes up on 127.0.0.1 only and is unreachable remotely.
    # Packaged (desktop.rs::spawn_gateway, LUCIDOS_PACKAGED=1) does NOT run
    # start_gateway, so it stays loopback-only. Defers to ~/.lucidos/network.toml
    # when the user set an explicit gateway bind there.
    apply_dev_gateway_bind
    # The gateway serves the picker from dist/ and passes LUCIDOS_STATIC_DIR
    # through to the engines it spawns so they serve dist/ too (set by swap_ports;
    # re-exported here for clarity). Asserted in `gateway` scope — the opt-out does
    # NOT apply, because this is the machine-global daemon that outlives the shell
    # and propagates these paths into every engine it spawns. That combination is
    # what made the 2026-07-26 pin self-perpetuating.
    assert_stack_not_worktree_pinned "$PROJECT_DIR" gateway || exit 1
    export LUCIDOS_STATIC_DIR="$FRONTEND_DIR/dist"

    # Reuse a healthy gateway already on the port (no -b restart), then make sure
    # this workspace's stack is up. ensure_workspace_engine_running decides
    # between a restart (the engine-only Apply path in gateway dev) and adopting
    # the engine already serving. The gateway's own surface lives behind the
    # sigil namespace (/~/, ADR 0014 §2).
    if [ -f "$gw_pidfile" ]; then
        local existing; existing="$(cat "$gw_pidfile" 2>/dev/null || true)"
        if [ -n "$existing" ] && kill -0 "$existing" 2>/dev/null \
           && curl -sk "$PROTO://localhost:$GATEWAY_PORT/~/api/v1/health" >/dev/null 2>&1; then
            echo "Reusing existing gateway (PID $existing) on port $GATEWAY_PORT"
            GATEWAY_PID="$existing"; ENGINE_SUPERVISOR_PID=""
            enable_clamshell_prevention
            adopt_into_gateway || exit 1
            ensure_workspace_engine_running
            return
        fi
    fi

    echo ""
    echo "Starting Lucidos workspace gateway..."
    if [ -f "$gw_log" ]; then
        local log_size; log_size=$(stat -f %z "$gw_log" 2>/dev/null || echo 0)
        if [ "$log_size" -gt 10485760 ]; then
            tail -c 1048576 "$gw_log" > "$gw_log.tmp" 2>/dev/null && mv "$gw_log.tmp" "$gw_log"
        fi
    fi
    ulimit -n 8192 2>/dev/null
    enable_clamshell_prevention

    # Safety net: ensure GATEWAY_PORT is free before binding. kill_stale_processes
    # already SIGUSR1'd + reaped a prior gateway on a `-b`; this clears a wedged or
    # leftover one (e.g. the first `-b` after upgrading from the old gateway-on-
    # the-user-port topology) so the fresh gateway doesn't hit AddrInUse.
    if ! port_is_free "$GATEWAY_PORT"; then
        local occ occ_cmd; occ=$(lsof -ti :"$GATEWAY_PORT" -sTCP:LISTEN 2>/dev/null | head -1 || true)
        occ_cmd=$(ps -p "$occ" -o comm= 2>/dev/null || true)
        # Only ever signal one of OUR binaries (never broad-kill a foreign
        # occupant on this fixed gateway port — CLAUDE.md kill safety). The
        # gateway (lucidos-gateway) or, on the first `-b` after an older
        # topology, an old `lucidos-engine --gateway`.
        if [ -n "$occ" ] && { [[ "$occ_cmd" == *lucidos-gateway* ]] || [[ "$occ_cmd" == *lucidos-engine* ]]; }; then
            echo "Reclaiming gateway port $GATEWAY_PORT from $occ_cmd (PID $occ)..."
            kill -USR1 "$occ" 2>/dev/null || true
            # Wait for the process, not the port. A draining gateway closes its
            # listener at once but holds the data dir's owner lock until it
            # exits, and the fresh gateway would exit 75 against it (ADR 0438).
            local _i; for _i in $(seq 1 10); do kill -0 "$occ" 2>/dev/null || break; sleep 0.3; done
            kill -0 "$occ" 2>/dev/null && kill -KILL "$occ" 2>/dev/null || true
        fi
    fi

    # Spawn the DEDICATED gateway supervisor (gateway_supervisor.sh), NOT the
    # engine's. It ignores SIGHUP/SIGINT/SIGTERM, so it survives this launcher's
    # shell exiting and the terminal closing — the gateway is a machine-global
    # daemon, not a child of this dev session (the orphan-on-terminal-close bug).
    # `disown` removes it from this shell's job table so web-dev.sh never `wait`s
    # on the daemon, and ENGINE_SUPERVISOR_PID is left EMPTY in gateway mode (the
    # gateway, not start_engine, owns the engines) so web-dev.sh's non-tty wait
    # falls back to the pidfile poll instead of blocking on the daemon.
    #
    # Two launches racing here are safe: the gateway that loses the data dir's
    # owner lock exits with the already-running code, its supervisor stops,
    # and gateway.pid keeps naming the winner (ADR 0438).
    ( unset "$AGENT_RUN_ENV"; run_gateway_supervised "$(gateway_data_dir)" "$gw_log" "$GATEWAY_BIN" ) &
    GATEWAY_SUPERVISOR_PID=$!
    disown "$GATEWAY_SUPERVISOR_PID" 2>/dev/null || true
    ENGINE_SUPERVISOR_PID=""

    local pid_deadline=$(( $(date +%s) + 5 ))
    while [ "$(date +%s)" -lt "$pid_deadline" ]; do
        if [ -s "$gw_pidfile" ]; then
            GATEWAY_PID="$(cat "$gw_pidfile" 2>/dev/null || true)"
            [ -n "$GATEWAY_PID" ] && kill -0 "$GATEWAY_PID" 2>/dev/null && break
        fi
        sleep 0.1
    done

    echo -n "Waiting for gateway"
    local ready=""
    for _ in $(seq 1 30); do
        if curl -sk "$PROTO://localhost:$GATEWAY_PORT/~/api/v1/health" >/dev/null 2>&1; then
            echo " ready!"; ready="yes"; break
        fi
        echo -n "."; sleep 1
    done
    if [ -z "$ready" ]; then
        echo ""
        echo "ERROR: gateway failed to start within 30s. Check logs:"
        tail -20 "$gw_log"
        kill -KILL "$GATEWAY_SUPERVISOR_PID" 2>/dev/null || true
        exit 1
    fi
    # Fresh gateway is up. Its boot adopts already-running engines + spawns
    # autostart workspaces, but it has not heard of this one yet. Register it,
    # then start it, exactly as the reuse path does.
    adopt_into_gateway || exit 1
    ensure_workspace_engine_running
}

# The gateway is now ONE shared machine-global process fronting every workspace,
# so stop.sh's stop_workspace does NOT tear it down — it POSTs the gateway's
# /stop control API to stop just the target workspace's engine (the gateway drops
# that stack so its supervisor won't respawn it; the registry entry survives, so
# the workspace stays listed in the picker as stopped). web-dev.sh's Ctrl+C trap
# (cleanup_processes) likewise leaves the shared gateway running. To stop the
# gateway itself: `kill $(cat "$(gateway_data_dir)/gateway.pid")`.

# ── shared build-watch (checkout-level singleton) ───────────────────────
# The build-watch produces the SHARED crates/lucidos-app/dist/ that every
# workspace of this checkout serves (each engine serves it directly via
# LUCIDOS_STATIC_DIR, ADR 0014). So exactly one runs per checkout, and the last
# consumer to go tears it down. Three locks in .build-watch/, one role each
# (ADR 0438):
#
#   watch.lock      the watcher's owner lock, held for its whole life. Taken
#                   blocking (build_watch_exec.pl), so probing it is sound.
#   start.lock      a launcher deciding whether to spawn one. It passes to the
#                   wrapper, which releases it once it owns watch.lock.
#   consumers.lock  shared by every engine serving this dist/ (the engine takes
#                   it itself), and by a launcher while it starts the watch.
#                   Teardown takes it exclusively without waiting.
#
# The paths match lucidos_owner_lock::build_watch, which the engine reads. A
# gateway test pins them (the_shell_reads_the_pidfiles_the_owners_write).
build_watch_dir()            { echo "$PROJECT_DIR/crates/lucidos-app/.build-watch"; }
build_watch_log()            { echo "$(build_watch_dir)/log"; }
build_watch_lock()           { echo "$(build_watch_dir)/watch.lock"; }
build_watch_pidfile()        { echo "$(build_watch_dir)/pid"; }
build_watch_start_lock()     { echo "$(build_watch_dir)/start.lock"; }
build_watch_consumers_lock() { echo "$(build_watch_dir)/consumers.lock"; }

# The descriptors the build-watch steps hold their locks on. Teardown reuses the
# start descriptor to wait on watch.lock, since nothing holds start.lock then.
BUILD_WATCH_CONSUMERS_FD=8
BUILD_WATCH_START_FD=9

# 0 while a watcher runs, 1 when none does, 2 when the probe failed.
build_watch_running() { lock_is_held "$(build_watch_lock)"; }

# The watcher's pid, while one runs.
build_watch_live_pid() { live_pid "$(build_watch_lock)" "$(build_watch_pidfile)"; }

# Stop a build-watch started before the watch lock existed. It holds no lock,
# so nothing else can see it, and a new watcher would run beside it. It is the
# one named by the old pidfile, running `node dev-build-watch.mjs`. Temporary
# measure: docs/temporary-measures.md § "Pre-lock engine pidfile fallback".
stop_pre_lock_build_watch() {
    [ ! -e "$(build_watch_lock)" ] || return 0
    local pid cmd
    pid="$(pidfile_pid_running "$(build_watch_pidfile)" node)"
    [ -n "$pid" ] || return 0
    cmd="$(ps -o command= -p "$pid" 2>/dev/null)" || return 0
    cmd="${cmd#* }"
    [ "${cmd%% *}" = dev-build-watch.mjs ] || return 0
    echo "Stopping a build-watch from before the watch lock (PID $pid)"
    kill "$pid" 2>/dev/null || true
}

# Tear down the shared build-watch when nothing serves this checkout any more.
#
# One exclusive, non-blocking try on consumers.lock decides it. Granted means
# no engine serves this dist/ and no launcher is starting the watch, and none
# can register until this lets go. So the decision and the kill cannot be split
# by a consumer arriving. A refusal always says so.
#
# It waits for the watcher to exit before letting go, so a launch that
# registers next never reuses a dying watch. Callers that just stopped their
# own engine wait for it to exit first, since a draining engine still holds its
# consumer lock.
teardown_shared_build_watch_if_idle() {
    [ -d "$(build_watch_dir)" ] || return 0
    local rc=0
    flock_try "$BUILD_WATCH_CONSUMERS_FD" "$(build_watch_consumers_lock)" ex || rc=$?
    if [ "$rc" -eq 1 ]; then
        echo "Keeping the shared frontend build-watch: an engine or a launch still uses $PROJECT_DIR"
        return 0
    fi
    if [ "$rc" -ne 0 ]; then
        echo "Keeping the shared frontend build-watch: could not read its consumer lock"
        return 0
    fi
    local pid
    pid="$(build_watch_live_pid)"
    if [ -n "$pid" ]; then
        echo "Stopping the shared frontend build-watch (PID $pid): nothing serves this checkout any more"
        kill "$pid" 2>/dev/null || true
        if flock_open "$BUILD_WATCH_START_FD" "$(build_watch_lock)" ex; then
            flock_close "$BUILD_WATCH_START_FD"
        fi
    fi
    flock_close "$BUILD_WATCH_CONSUMERS_FD"
}

# ── _resolve_npm_install_root ─────────────────────────────────────────
# For npm-workspace members, deps are hoisted to the workspace root —
# the per-package node_modules dir often only holds Vite's cache (or
# nothing on a fresh git worktree). Walk up looking for an ancestor
# package.json that lists this dir as a workspace member; if found,
# use that ancestor as the install root. Echoes the resolved path.
_resolve_npm_install_root() {
    local dir="$1"
    local p="$dir"
    while [ "$p" != "/" ] && [ -n "$p" ]; do
        local parent
        parent="$(dirname "$p")"
        [ "$parent" = "$p" ] && break
        p="$parent"
        if [ -f "$p/package.json" ] && grep -qE '"workspaces"[[:space:]]*:' "$p/package.json" 2>/dev/null; then
            echo "$p"
            return 0
        fi
    done
    echo "$dir"
}

# ── _deps_fingerprint ──────────────────────────────────────────────────
# Content fingerprint of the inputs that decide an npm install at this root:
# every package.json `_deps_manifest_files` lists, plus the root lockfile. Content-only by design — unlike an mtime comparison, a
# no-op rewrite (a git checkout, `git worktree add`, or a CC change apply
# rewrites package.json and bumps its mtime without changing a byte) yields
# an identical fingerprint, so it can't trigger a spurious reinstall that
# would deadlock an engine-only restart while a frontend is running.
# cksum is portable (macOS + Linux) and sufficient for change detection.
_deps_fingerprint() {
    local root="$1"
    {
        _deps_manifest_files "$root" \
            | LC_ALL=C sort | while IFS= read -r f; do cat "$root/$f" 2>/dev/null; done
        [ -f "$root/package-lock.json" ] && cat "$root/package-lock.json"
    } | cksum
}

# _deps_manifest_files ROOT: every package.json the install reads, relative to
# ROOT. In a git checkout, ask git: a nested checkout under ROOT (a coding
# agent's `.claude/worktrees/<name>`) holds another tree's manifests, never this
# install's. Engine worktrees share main's stamp, so both must count alike.
_deps_manifest_files() {
    local root="$1"
    if [ "$(git -C "$root" rev-parse --show-toplevel 2>/dev/null)" = "$(cd "$root" && pwd -P)" ]; then
        git -C "$root" ls-files --cached --others --exclude-standard -- ':(glob)**/package.json'
        return
    fi
    (cd "$root" && find . \
        \( -name node_modules -o -name .git -o -name target \
           -o -name .lucidos -o -name dist \) -prune \
        -o -name package.json -print 2>/dev/null) | sed 's|^\./||'
}

# ── ensure_npm_deps ───────────────────────────────────────────────────
# Run npm install if node_modules is missing or incomplete (no npm install
# marker), or the dependency fingerprint changed (see _deps_fingerprint — content, not mtime). Refuses while
# THIS checkout's build-watch runs: npm workspaces hoists deps to one shared
# node_modules tree, and replacing it under a running build corrupts that build
# (a blank page in the browser). The watch lock is per checkout, so a watch in
# another checkout (e.g. a CC worktree) does not block.
# Usage: ensure_npm_deps "path/to/package" "label"
ensure_npm_deps() {
    local dir="$1"
    local label="${2:-dependencies}"
    local needs_install=""

    # Resolve the actual install marker. In npm-workspace setups the per-package
    # node_modules dir is essentially Vite cache — the real install marker is
    # the workspace-root node_modules. Without this, fresh git worktrees fail
    # the "node_modules missing" check even though `npm install` already ran at
    # the root and hoisted everything.
    local install_root
    install_root="$(_resolve_npm_install_root "$dir")"

    local stamp="$install_root/node_modules/.lucidos-deps-stamp"

    if [ ! -d "$install_root/node_modules" ]; then
        needs_install="node_modules missing"
    elif [ ! -f "$install_root/node_modules/.package-lock.json" ]; then
        # npm writes this marker at the end of an install, so its absence is one
        # that never finished, such as an `npm ci` killed mid-wipe. Stamping it
        # as current would trust a tree with packages missing.
        needs_install="node_modules incomplete"
    elif [ ! -f "$stamp" ]; then
        # node_modules exists but predates this fingerprint scheme (installed by
        # an older script, or `npm install` run directly). Trust it as current
        # and stamp it now rather than forcing a reinstall — forcing one here
        # would deadlock an engine-only restart whenever a frontend is running.
        # Genuine content changes are detected normally once the stamp exists.
        _deps_fingerprint "$install_root" > "$stamp" 2>/dev/null || true
    elif [ "$(_deps_fingerprint "$install_root")" != "$(cat "$stamp" 2>/dev/null)" ]; then
        needs_install="dependencies changed"
    fi

    if [ -n "$needs_install" ]; then
        local watch_rc=0 holder
        build_watch_running || watch_rc=$?
        if [ "$watch_rc" -ne 1 ]; then
            holder="this checkout's build-watch (PID $(build_watch_live_pid))"
            [ "$watch_rc" -eq 0 ] || holder="this checkout's build-watch, whose lock could not be read,"
            # Two non-disruptive paths deliberately keep the running frontend
            # alive and so MUST NOT touch its shared node_modules:
            #   • --engine-only (CC Apply switch): a per-workspace restart never
            #     touches the shared build-watch.
            #   • --engine-build (Apply background rebuild): just compiles the new
            #     on-disk binary and exits — no restart, no teardown at all.
            # For BOTH, generate_build_outputs runs, so a hard-fail here kills the whole build
            # or restart. So skip the install and keep the working node_modules.
            # The stamp stays un-updated, so the next full restart re-detects the
            # change. Outside these two paths a running build-watch IS a genuine
            # conflict, so hard-fail and say so.
            if [ -n "${ENGINE_ONLY:-}" ] || [ -n "${ENGINE_BUILD_ONLY:-}" ]; then
                local skip_ctx="engine restart"
                [ -n "${ENGINE_BUILD_ONLY:-}" ] && skip_ctx="background build"
                echo "" >&2
                echo "WARNING: $label changed ($needs_install) but $holder is running." >&2
                echo "Skipping install to keep it alive; the $skip_ctx will still proceed." >&2
                echo "The build-watch installs the new dependencies before its next build." >&2
                echo "" >&2
                return 0
            fi
            echo "" >&2
            echo "ERROR: $label install needed ($needs_install), but $holder is running." >&2
            echo "Installing now would corrupt the build it is running (blank page)." >&2
            echo "Stop every workspace of this checkout first:  ./scripts/stop.sh -w <name>" >&2
            echo "" >&2
            exit 1
        fi
        echo "Installing $label ($needs_install)..."
        # `npm ci` (not `npm install`) so the install is strict + deterministic:
        # it installs exactly the committed lockfile, verifies integrity hashes,
        # and ERRORS on any manifest↔lock drift instead of silently rewriting the
        # lock. Run from the workspace root (install_root) — npm workspaces hoist
        # deps there, and `npm ci` resolves the whole tree from the root lockfile.
        # Only reached when deps genuinely changed AND no frontend is running
        # (guarded above), so the wipe-and-reinstall `npm ci` does is safe here.
        # A deliberate dep change is `npm install <pkg>` run by hand (updates the
        # lock); this automated path just restores that frozen state.
        (cd "$install_root" && npm ci)
        # Record what we just installed so the next check compares content.
        _deps_fingerprint "$install_root" > "$stamp" 2>/dev/null || true
    fi
}

# ── ensure_frontend_deps ───────────────────────────────────────────────
# Run npm install if node_modules is missing or package.json has changed
# since the last install. Safe to call from both web-dev and tauri-dev.
ensure_frontend_deps() {
    ensure_npm_deps "$FRONTEND_DIR" "frontend dependencies"
}

# ── generate_build_outputs ─────────────────────────────────────────────
# Write every generated file (scripts/codegen.sh): the Rust-rendered TS, CSS
# and JSON, then the SDK bundles in packages/lucidos-sdk/dist/, which the
# engine serves (/api/v1/sdk.js and the scripts beside it). Runs after
# build_or_find_engine. When that just compiled, a codegen cache miss renders
# with the lucidos-codegen it built beside the engine, for free.
#
# REUSED_CODEGEN_BIN names a lucidos-codegen the e2e engine cache matched to
# this tree, for a caller that reused a build rather than compiling one.
generate_build_outputs() {
    # SDK deps are hoisted to root node_modules by npm workspaces
    ensure_npm_deps "$PROJECT_DIR" "workspace dependencies"
    echo "Generating build outputs..."
    local codegen_bin
    codegen_bin="$PROJECT_DIR/target/$(engine_build_profile)/lucidos-codegen"
    local -a bin_args=()
    if [ -n "${REUSED_CODEGEN_BIN:-}" ]; then
        bin_args=(--reused-bin "$REUSED_CODEGEN_BIN")
    elif [ -n "${BUILD:-}" ] && [ -x "$codegen_bin" ]; then
        bin_args=(--bin "$codegen_bin")
    fi
    "$PROJECT_DIR/scripts/codegen.sh" ${bin_args[@]+"${bin_args[@]}"}
}

# ── start_vite ──────────────────────────────────────────────────────────
# Install npm deps if needed, then ensure the shared `vite build --watch` is
# running so dist/ exists + rebuilds on source change. ADR 0014: the engine
# serves dist/ DIRECTLY (LUCIDOS_STATIC_DIR) — there is no `vite preview` and no
# live dev server. ENGINE_ONLY restarts never reach here (web-dev.sh exits before
# start_vite), so a CC Apply leaves the running build-watch untouched.
start_vite() {
    ensure_frontend_deps
    # End-user launchers (scripts/run.sh) set LUCIDOS_FRONTEND_ONESHOT=1: an
    # installed user never edits source, so build dist/ ONCE and leave no
    # long-lived watcher behind. Every dev caller leaves it unset and gets the
    # checkout-level `vite build --watch` singleton exactly as before — this
    # branch is the ONLY behavioural difference between run.sh and web-dev.sh.
    if [ -n "${LUCIDOS_FRONTEND_ONESHOT:-}" ]; then
        build_frontend_oneshot
    else
        start_frontend_built
    fi
}

# ── build_frontend_oneshot ──────────────────────────────────────────────
# End-user (scripts/run.sh) frontend build: produce the served dist/ with a
# SINGLE `vite build` and leave NO long-lived watcher behind (an installed user
# never edits source). The engine serves dist/ directly via LUCIDOS_STATIC_DIR
# (ADR 0014), exactly like the dev path — only the "keep rebuilding on change"
# watcher is dropped. Mirrors the one-shot build scripts/lib/e2e.sh uses.
#
# Always rebuilds: run.sh passes -b on every launch (so a `git pull` + restart
# picks up new source), and a fresh `vite build` here is sub-second. It starts
# no watcher, so it takes none of the build-watch locks.
build_frontend_oneshot() {
    echo "Building frontend (one-shot vite build)..."
    # Drop the atomic-publish scratch dirs a prior `vite build --watch` may have
    # left in this checkout; a plain `vite build` (no LUCIDOS_ATOMIC_DIST) empties
    # and writes dist/ itself, so dist/ is left for vite to manage.
    rm -rf "${FRONTEND_DIR:?}/dist.staging" "$FRONTEND_DIR/dist.prev"
    (cd "$FRONTEND_DIR" && npx vite build) || {
        echo "ERROR: frontend build failed" >&2
        exit 1
    }
    if [ ! -f "$FRONTEND_DIR/dist/index.html" ]; then
        echo "ERROR: vite build did not produce $FRONTEND_DIR/dist/index.html" >&2
        exit 1
    fi
}

# Built frontend: the build-watch (dev-build-watch.mjs) runs a clean `vite build`
# per change, producing a bundled, content-hashed dist/ served DIRECTLY by the
# engine via LUCIDOS_STATIC_DIR (ADR 0014 — no `vite preview`). The SW caches
# /assets/* so an iOS PWA resumes instantly, and the "New version available" toast
# (driven by the per-build sw.js stamp) signals when a rebuild is ready to reload.
# Note: this skips `tsc --noEmit` — type errors surface at the explicit build / in
# CC harden.
#
# The build-watch + dist/ are a CHECKOUT-LEVEL singleton shared by every workspace
# of this checkout, held by locks (see the shared build-watch section above).
# This launch reuses a running watch and starts one only when none runs. A live
# watch already serves current source, so nothing here replaces it.
#
# Holds consumers.lock shared until the watch it reused or started has
# published dist/, so no teardown can stop that watch first. The engine took
# its own consumer lock at boot. Holds start.lock while it decides, so two
# launches never both spawn one.
start_frontend_built() {
    local bw_log
    bw_log="$(build_watch_log)"
    mkdir -p "$(build_watch_dir)"

    flock_open "$BUILD_WATCH_CONSUMERS_FD" "$(build_watch_consumers_lock)" sh || exit 1
    if ! flock_open "$BUILD_WATCH_START_FD" "$(build_watch_start_lock)" ex; then
        flock_close "$BUILD_WATCH_CONSUMERS_FD"
        exit 1
    fi

    stop_pre_lock_build_watch
    local watch_rc=0 spawned=""
    build_watch_running || watch_rc=$?
    case "$watch_rc" in
        0)
            BUILD_WATCH_PID="$(build_watch_live_pid)"
            flock_close "$BUILD_WATCH_START_FD"
            echo "Reusing shared frontend build-watch (PID $BUILD_WATCH_PID) serving $FRONTEND_DIR/dist."
            ;;
        1)
            echo "Building frontend (fresh vite build per change)..."
            # The wrapper inherits start.lock and releases it once it owns
            # watch.lock. It clears dist/ as the owner, publishes its pid, then
            # execs the watcher, which LUCIDOS_ATOMIC_DIST-publishes each build.
            # Output goes to a log (not /dev/null) so a build failure is one
            # `tail` away. LUCIDOS_CLI_BIN lets the watcher raise a notification
            # when a build fails. Absolute, the convention `desktop.rs`
            # `spawn_gateway` uses. LUCIDOS_WORKSPACE is already exported by
            # `resolve_workspace`, and the CLI needs both.
            local cli_bin="${ENGINE_BIN:+${ENGINE_BIN%/*}/lucidos}"
            # The watcher must not inherit this launch's consumer lock, or it
            # would count itself as a consumer for its whole life.
            (flock_close "$BUILD_WATCH_CONSUMERS_FD" && cd "$FRONTEND_DIR" \
                && unset "$AGENT_RUN_ENV" && LUCIDOS_CLI_BIN="$cli_bin" exec perl "$WORKSPACE_LIB_DIR/build_watch_exec.pl" \
                "$FRONTEND_DIR" "$(build_watch_lock)" "$(build_watch_pidfile)" "$BUILD_WATCH_START_FD" \
                node dev-build-watch.mjs) > "$bw_log" 2>&1 &
            BUILD_WATCH_PID=$!
            spawned=1
            flock_close "$BUILD_WATCH_START_FD"
            ;;
        *)
            flock_close "$BUILD_WATCH_START_FD"
            flock_close "$BUILD_WATCH_CONSUMERS_FD"
            echo "ERROR: could not read $(build_watch_lock), so this launch cannot tell whether a build-watch runs." >&2
            exit 1
            ;;
    esac

    echo -n "Waiting for the frontend build (log: $bw_log)"
    local build_deadline=$((SECONDS + 180))
    while (( SECONDS < build_deadline )); do
        # A spawned watcher counts only once it owns watch.lock: before that,
        # dist/index.html may be the previous session's, which it is about to
        # delete.
        if [ -f "$FRONTEND_DIR/dist/index.html" ] \
            && { [ -z "$spawned" ] || [ "$(build_watch_live_pid)" = "$BUILD_WATCH_PID" ]; }; then
            echo " done!"
            break
        fi
        if [ -n "$spawned" ] && ! kill -0 "$BUILD_WATCH_PID" 2>/dev/null; then
            echo ""
            echo "ERROR: frontend build-watch exited before producing dist/index.html. See $bw_log" >&2
            flock_close "$BUILD_WATCH_CONSUMERS_FD"
            exit 1
        fi
        echo -n "."
        sleep 1
    done
    flock_close "$BUILD_WATCH_CONSUMERS_FD"
    if (( SECONDS >= build_deadline )); then
        echo ""
        echo "ERROR: the frontend build did not complete within 180s. See $bw_log" >&2
        if [ -n "$spawned" ]; then kill "$BUILD_WATCH_PID" 2>/dev/null || true; fi
        exit 1
    fi
}

# ── sleep_prevention_status ─────────────────────────────────────────────
sleep_prevention_status() {
    local pmset_out status=""
    pmset_out="$(pmset -g 2>/dev/null || true)"
    if echo "$pmset_out" | grep -q "disablesleep.*1"; then
        status="Blocked (lid-close safe)"
    else
        status="WARNING: lid-close will sleep Mac (restart from terminal once)"
    fi
    if echo "$pmset_out" | grep -q "lowpowermode.*1"; then
        status="$status — Low Power Mode ON (may override)"
    fi
    echo "$status"
}

# ── show_banner ─────────────────────────────────────────────────────────
# Print startup info. $1 = "web" or "tauri".
show_banner() {
    local mode="${1:-web}"

    # Detect network
    local local_ip ts_hostname
    local_ip=$(ipconfig getifaddr en0 2>/dev/null || echo "unknown")
    ts_hostname=$(tailscale status --self --json 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['Self'].get('DNSName','').rstrip('.'))" 2>/dev/null || echo "")

    echo ""
    echo "========================================"
    if [ "$mode" = "tauri" ]; then
        echo "  Lucidos Tauri dev ready"
    elif [ "$mode" = "engine-only" ]; then
        echo "  Lucidos engine restarted"
    else
        echo "  Lucidos dev server ready"
    fi
    echo "  Workspace:   $WORKSPACE"
    # Dev topology (ADR 0014 §4): the gateway is the only network door, and it
    # serves the workspace under /<slug>/ with the picker at /~/. The engine it
    # spawns binds loopback and plain http, so its port answers on this machine
    # alone. Legacy direct-engine mode (no gateway) prints the engine URLs,
    # because there the engine IS the front.
    if [ -n "${GATEWAY_MODE:-}" ] && [ -n "${GATEWAY_WS_ID:-}" ]; then
        echo "  Local:       $PROTO://localhost:$GATEWAY_PORT/$GATEWAY_WS_ID/"
        echo "  Network:     $PROTO://$local_ip:$GATEWAY_PORT/$GATEWAY_WS_ID/"
        if [ -n "$ts_hostname" ]; then
            echo "  Tailscale:   $PROTO://$ts_hostname:$GATEWAY_PORT/$GATEWAY_WS_ID/"
        fi
        echo "  Picker:      $PROTO://localhost:$GATEWAY_PORT/~/"
        echo "  Engine port: http://localhost:$ENGINE_PORT/  (loopback only)"
    else
        echo "  Local:       $PROTO://localhost:$ENGINE_PORT/"
        echo "  Network:     $PROTO://$local_ip:$ENGINE_PORT/"
        if [ -n "$ts_hostname" ]; then
            echo "  Tailscale:   $PROTO://$ts_hostname:$ENGINE_PORT/"
        fi
    fi
    # ADR 0014: the engine serves the built dist/ directly (no Vite in the
    # serving path) in every mode, including tauri-dev (the dev window loads
    # dist/ through the gateway or the engine). An --engine-only restart leaves the running
    # build-watch untouched.
    if [ "$mode" = "engine-only" ]; then
        echo "  Frontend:    built dist/ served by the engine (unchanged)"
    else
        echo "  Frontend:    built dist/ served by the engine (fresh vite build per change)"
    fi
    echo "  PostgreSQL:  shared localhost:$PG_PORT / $(workspace_database_name)"
    echo "  Engine:      Native macOS"
    echo "  Sleep:       $(sleep_prevention_status)"
    echo "  Log:         $ENGINE_LOG"
    echo "========================================"
    echo ""
}

# ── cleanup_processes ───────────────────────────────────────────────────
# Stop frontend (always) and engine (only if we started it). Called from trap handlers.
cleanup_processes() {
    echo ""
    release_sleep_lock
    # The build-watch is a checkout-level singleton. This workspace's engine
    # keeps running and keeps its consumer lock, so this tears the watch down
    # only when that engine has already gone.
    teardown_shared_build_watch_if_idle
    echo "Engine still running for workspace: $WORKSPACE (port $ENGINE_PORT)"
    echo "Stop with: ./scripts/stop.sh -w $WORKSPACE"
    echo "Shared PostgreSQL still running (container $(shared_pg_container)) for all workspaces."
    if _legacy_postgres_exists; then
        echo "Legacy per-workspace PostgreSQL is preserved for rollback. Decommission after verifying with:"
        echo "  ./scripts/decommission-legacy-postgres.sh -w $WORKSPACE"
    fi
}
