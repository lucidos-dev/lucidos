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
        */.lucidos/worktrees/*|*/.lucidos/worktrees) return 0 ;;
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
# directly (legacy direct-engine model, ADR 0014) and never starts a gateway, so
# it only ever hits the `stack` scope, where the opt-out applies.
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
# Parse -w, -b, -r, -h flags. Sets WORKSPACE, BUILD, RELEASE, BUILT.
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
    # The engine serves the built dist/ DIRECTLY (ADR 0014) — `vite build --watch`
    # rebuilds it on source change; the SW caches bundled /assets/* so an iOS PWA
    # resumes instantly. BUILT stays set for back-compat with callers that read it
    # (the old `--hmr` live-dev-server path was removed — there is no Vite proxy).
    # shellcheck disable=SC2034 # documented output of this parser, kept for back-compat callers
    BUILT="1"
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
            --built) shift ;;   # accepted for back-compat; BUILT is already 1 (always built now)
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
# missing, and sets ENGINE_PIDFILE / FRONTEND_PIDFILE / ENGINE_LOG / PG_NAME.
# Use from start scripts (web-dev.sh, tauri-dev.sh, e2e). For stop / status
# / tail, use resolve_workspace_path instead.
resolve_workspace() {
    # Bare name (no /) → ~/workspaces/<name>
    if [[ "$WORKSPACE" != */* ]]; then
        WORKSPACE="$HOME/workspaces/$WORKSPACE"
    fi

    # Create if needed, then resolve to absolute path
    if [ ! -d "$WORKSPACE" ]; then
        echo "Creating workspace: $WORKSPACE"
        mkdir -p "$WORKSPACE"
    fi
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
    ENGINE_PIDFILE="$WORKSPACE/.lucidos/engine.pid"
    FRONTEND_PIDFILE="$WORKSPACE/.lucidos/frontend.pid"
    # NOTE: `vite build --watch` is NOT workspace-scoped — it produces the SHARED
    # crates/lucidos-app/dist/ that every workspace of this checkout serves
    # (the engine serves it directly via LUCIDOS_STATIC_DIR, ADR 0014), so it's a
    # checkout-level singleton tracked by build_watch_pidfile (below), not a
    # per-workspace pid. A legacy per-workspace build-watch.pid from pre-singleton
    # runs is cleaned up by kill_stale_processes / stop.sh.
    ENGINE_LOG="$WORKSPACE/.lucidos/engine.log"

    # Compute a short name for the postgres container from workspace path
    PG_NAME=$(printf '%s' "$WORKSPACE" | cksum | awk '{print $1}')
}

# ── detect_tls ──────────────────────────────────────────────────────────
# Check for TLS certs. Checks .certs/ dir first, then falls back to
# LUCIDOS_TLS_CERT/KEY env vars (needed in worktrees where .certs/ is gitignored).
# Sets PROTO, exports LUCIDOS_TLS_CERT/KEY. Persists PROTO= to the ports
# file so the CLI and cross-workspace callers know the protocol.
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
    local ports_file="$WORKSPACE/.lucidos/ports"
    if [ -f "$ports_file" ]; then
        echo "PROTO=$PROTO" >> "$ports_file"
    fi
}

# ── setup_postgres ──────────────────────────────────────────────────────
# Start/verify the ONE shared Docker Postgres cluster and ensure this
# workspace's database exists. Legacy per-workspace containers/volumes are kept
# intact; if present and this shared database has not been verified, they are
# dumped/restored into the shared cluster first.
shared_pg_container() { echo "${LUCIDOS_SHARED_PG_CONTAINER:-lucidos-pg-shared}"; }
shared_pg_volume()    { echo "${LUCIDOS_SHARED_PG_VOLUME:-lucidos-pg-data-shared}"; }

workspace_database_name() {
    local id
    id="$(workspace_slug)"
    echo "lucidos_$id"
}

workspace_database_url() {
    echo "postgres://lucidos:lucidos@localhost:$PG_PORT/$(workspace_database_name)"
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
        # --shm-size=1g: Postgres builds the pgvector HNSW index with parallel
        # maintenance workers that use POSIX shared memory under /dev/shm. Docker's
        # 64m default overflows ("could not resize shared memory segment ... No
        # space left on device") when restoring/migrating a workspace with a
        # sizeable memory_entries table, aborting the migration and leaving the
        # workspace stuck on the gateway's "Workspace starting…" page. 1g is a
        # generous, safe ceiling for a personal machine.
        #
        # max_connections=500: this is ONE shared cluster for every workspace on
        # the machine (ADR 0014 §6/§7), and each engine opens a pool of up to 50
        # connections (construction.rs). Postgres' default 100 is exhausted by
        # just two busy workspaces, so a third fails to start with "sorry, too
        # many clients already". 500 fits ~10 concurrent engines. Keep this in
        # lockstep with crates/lucidos-gateway/src/postgres.rs.
        if ! docker run -d \
            --name "$container" \
            --restart unless-stopped \
            --shm-size=1g \
            -p "127.0.0.1:$PG_PORT:5432" \
            -e POSTGRES_USER=lucidos \
            -e POSTGRES_PASSWORD=lucidos \
            -e POSTGRES_DB=postgres \
            -v "$volume:/var/lib/postgresql" \
            --label "lucidos.shared-postgres=true" \
            pgvector/pgvector:pg18 \
            postgres -c max_connections=500 >/dev/null; then
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
    local ready_streak=0 ready_needed=3
    echo -n "Waiting for shared PostgreSQL"
    for _ in {1..90}; do
        if docker exec "$container" pg_isready -U lucidos -d postgres -h 127.0.0.1 -p 5432 >/dev/null 2>&1; then
            ready_streak=$((ready_streak + 1))
            if [ "$ready_streak" -ge "$ready_needed" ]; then
                echo " ready!"
                return 0
            fi
        elif [ "$ready_streak" -gt 0 ]; then
            # It answered, then stopped answering: that is the init server going
            # away. Start the count over rather than proceeding on a stale yes.
            ready_streak=0
        fi
        echo -n "."
        sleep 1
    done
    echo ""
    echo "ERROR: shared PostgreSQL did not become ready" >&2
    return 1
}

_shared_database_exists() {
    local db="$1" container
    container="$(shared_pg_container)"
    docker exec "$container" psql -U lucidos -d postgres -tAc \
        "SELECT 1 FROM pg_database WHERE datname=$(_shared_pg_literal "$db")" 2>/dev/null | grep -qx 1
}

_create_shared_database() {
    local db="$1" ident
    ident="$(_shared_pg_ident "$db")" || return 1
    docker exec "$(shared_pg_container)" psql -U lucidos -d postgres -v ON_ERROR_STOP=1 -c \
        "CREATE DATABASE $ident OWNER lucidos" >/dev/null
}

_drop_shared_database() {
    local db="$1" ident lit
    ident="$(_shared_pg_ident "$db")" || return 1
    lit="$(_shared_pg_literal "$db")"
    docker exec "$(shared_pg_container)" psql -U lucidos -d postgres -c \
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname=$lit AND pid <> pg_backend_pid()" >/dev/null 2>&1 || true
    docker exec "$(shared_pg_container)" psql -U lucidos -d postgres -c \
        "DROP DATABASE IF EXISTS $ident" >/dev/null
}

_verify_shared_pg_database() {
    local db="$1"
    docker exec "$(shared_pg_container)" psql -U lucidos -d "$db" -tAc "SELECT 1" 2>/dev/null | grep -qx 1
}

_ensure_shared_workspace_database() {
    local db="$1"
    if _shared_database_exists "$db"; then
        _verify_shared_pg_database "$db" || return 1
        echo "PostgreSQL shared database ready: $db (container $(shared_pg_container), port $PG_PORT)"
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

# One-time MAJOR-version migration (PG 17 → 18). The named volume was created
# when the image was pgvector/pgvector:pg17, whose PGDATA was the volume root
# (/var/lib/postgresql/data). PG 18's image relocated PGDATA to
# /var/lib/postgresql/18/docker and mounts the volume at the parent
# /var/lib/postgresql, so the old cluster cannot be opened in place — a logical
# dump/restore is required.
#
# Boots a throwaway PG 17 server on the old volume, pg_dump's the `lucidos`
# database to <workspace>/.lucidos, verifies the archive, then removes the old
# volume so _start_postgres_container can initdb a fresh PG 18 cluster into the
# (recreated) named volume. _restore_postgres_major_dump_if_pending finishes the
# restore once the PG 18 server is up.
#
# Idempotent: no-op when the volume is absent (fresh install) or already in PG 18
# layout (/18/docker/PG_VERSION present). The dump archive is kept (never
# deleted) so the migration is recoverable. Returns non-zero ONLY when an
# in-progress migration could not complete safely (caller aborts the start).
_migrate_postgres_major_if_needed() {
    local volume_name="lucidos-pg-data-$PG_NAME"
    local image_peek="pgvector/pgvector:pg18"   # always present once compose has run
    local pending="$WORKSPACE/.lucidos/pg-major-migrate.pending.dump"

    # No volume → fresh install; compose will initdb a PG 18 cluster.
    docker volume inspect "$volume_name" >/dev/null 2>&1 || return 0

    # Already PG 18 layout → nothing to do.
    if docker run --rm -v "$volume_name:/v" "$image_peek" \
        sh -c 'test -f /v/18/docker/PG_VERSION' >/dev/null 2>&1; then
        return 0
    fi

    # Old layout keeps PGDATA at the volume root. Read its catalog major version.
    local old_ver
    old_ver=$(docker run --rm -v "$volume_name:/v" "$image_peek" \
        sh -c 'cat /v/PG_VERSION 2>/dev/null' | tr -d '[:space:]')
    # No PG_VERSION at root and no 18/docker → empty/foreign volume; let compose
    # initdb fresh rather than risk a destructive guess.
    [ -n "$old_ver" ] || return 0
    if [ "$old_ver" -ge 18 ] 2>/dev/null; then
        return 0
    fi
    # Boot the image matching the volume's actual catalog version (a newer
    # server cannot open an older cluster). The project has only shipped PG 17,
    # but deriving the tag keeps the migration correct for any past major.
    local image_old="pgvector/pgvector:pg${old_ver}"

    echo ""
    echo "Migrating PostgreSQL cluster PG $old_ver → 18 (logical dump/restore)."
    echo "(One-time. PG 18 relocated PGDATA, so the PG $old_ver cluster cannot be opened in place.)"
    echo "  Volume:  $volume_name"
    echo "  Archive: $pending"

    # Release the volume from the workspace container before booting a temp one.
    docker rm -f "lucidos-pg-$PG_NAME" >/dev/null 2>&1 || true

    local tmp="lucidos-pg-migrate-$PG_NAME"
    docker rm -f "$tmp" >/dev/null 2>&1 || true

    echo "  Booting temporary PG $old_ver to dump the event store..."
    if ! docker run -d --name "$tmp" \
        -v "$volume_name:/var/lib/postgresql/data" \
        -e POSTGRES_USER=lucidos -e POSTGRES_PASSWORD=lucidos -e POSTGRES_DB=lucidos \
        "$image_old" >/dev/null 2>&1; then
        echo "ERROR: could not start temporary PG $old_ver for migration (old volume intact)." >&2
        return 1
    fi

    local ready=""
    for _ in $(seq 1 60); do
        if docker exec "$tmp" pg_isready -U lucidos >/dev/null 2>&1; then ready=1; break; fi
        sleep 1
    done
    if [ -z "$ready" ]; then
        echo "ERROR: temporary PG $old_ver did not become ready; aborting (old volume intact)." >&2
        docker logs --tail 30 "$tmp" >&2 || true
        docker rm -f "$tmp" >/dev/null 2>&1 || true
        return 1
    fi

    mkdir -p "$WORKSPACE/.lucidos"
    echo "  Dumping 'lucidos' database (custom format)..."
    if ! docker exec "$tmp" pg_dump -U lucidos -d lucidos -Fc -f /tmp/lucidos.dump; then
        echo "ERROR: pg_dump failed; aborting (old volume intact)." >&2
        docker rm -f "$tmp" >/dev/null 2>&1 || true
        return 1
    fi

    # Verify the archive lists a non-empty TOC before trusting it.
    if ! docker exec "$tmp" sh -c 'pg_restore -l /tmp/lucidos.dump | grep -q .'; then
        echo "ERROR: dump archive verification failed; aborting (old volume intact)." >&2
        docker rm -f "$tmp" >/dev/null 2>&1 || true
        return 1
    fi

    if ! docker cp "$tmp:/tmp/lucidos.dump" "$pending" >/dev/null 2>&1; then
        echo "ERROR: could not copy dump out of temporary container; aborting (old volume intact)." >&2
        docker rm -f "$tmp" >/dev/null 2>&1 || true
        return 1
    fi
    docker rm -f "$tmp" >/dev/null 2>&1 || true

    # Dump is verified and on the host — safe to drop the old volume so compose
    # recreates it in the PG 18 layout. The archive at $pending is the backup.
    if ! docker volume rm "$volume_name" >/dev/null 2>&1; then
        echo "ERROR: could not remove old volume $volume_name after dumping." >&2
        echo "       Dump archive preserved at $pending" >&2
        return 1
    fi
    echo "  PG $old_ver cluster dumped; old volume removed. A fresh PG 18 cluster will be initialized."
}

# Finish a pending PG 17 → 18 migration: restore the dumped `lucidos` database
# into the freshly-initialized PG 18 cluster. No-op when no migration is pending.
# Called by _start_postgres_container once the PG 18 server is ready and BEFORE
# the vector extension is (re)created — the dump already carries it.
_restore_postgres_major_dump_if_pending() {
    local pending="$WORKSPACE/.lucidos/pg-major-migrate.pending.dump"
    [ -f "$pending" ] || return 0

    local container="lucidos-pg-$PG_NAME"
    local archived
    archived="$WORKSPACE/.lucidos/pg-major-migrate.restored-$(date +%Y%m%d%H%M%S).dump"

    # Guard against double-apply: only restore into a pristine cluster. If the
    # event store already exists (a prior restore that completed but failed to
    # archive the marker), skip and archive the dump.
    if docker exec "$container" psql -U lucidos -d lucidos -tAc \
        "SELECT to_regclass('public.events') IS NOT NULL" 2>/dev/null | grep -q t; then
        echo "PG 18 cluster already populated; skipping restore. Dump archived at $archived"
        mv "$pending" "$archived" 2>/dev/null || true
        return 0
    fi

    echo "Restoring event store into PG 18 from $pending ..."
    if ! docker cp "$pending" "$container:/tmp/restore.dump" >/dev/null 2>&1; then
        echo "ERROR: could not copy dump into $container; PG 17 archive kept at $pending" >&2
        return 1
    fi
    # -U lucidos is required: `docker exec` runs as the image's root user (it
    # bypasses the entrypoint's gosu drop to postgres), so without it pg_restore
    # connects as PG role "root", which does not exist.
    if ! docker exec "$container" pg_restore -U lucidos --no-owner --no-privileges \
        --exit-on-error -d lucidos /tmp/restore.dump; then
        echo "ERROR: pg_restore failed restoring the PG 17 → 18 migration." >&2
        echo "       The PG 17 dump is preserved at $pending — re-run after fixing the cause:" >&2
        echo "         docker cp $pending $container:/tmp/restore.dump" >&2
        echo "         docker exec $container pg_restore -U lucidos --no-owner --no-privileges -d lucidos /tmp/restore.dump" >&2
        return 1
    fi
    docker exec "$container" rm -f /tmp/restore.dump >/dev/null 2>&1 || true
    mv "$pending" "$archived" 2>/dev/null || true
    echo "PG 17 → 18 migration complete. Dump archived at $archived"
}

_start_postgres_container() {
    # Resolve a free PG host port BEFORE creating the container. The nominal
    # 5432+offset slot may be squatted by a sibling workspace's sticky container;
    # walk forward to a free (or our own) port so `docker compose up` doesn't
    # collide. This is decoupled from the vite/api offset on purpose — ports.sh
    # no longer drifts the user-facing ports for a PG conflict, so it's resolved
    # here instead. `_pg_port_is_ours_or_free` accepts our own container (we
    # recreate it below). Re-export so docker-compose and DATABASE_URL pick it up.
    local pg_steps=0
    while ! _pg_port_is_ours_or_free "$PG_PORT"; do
        local squatter
        squatter=$(docker ps --filter "publish=$PG_PORT" --format "{{.Names}}" 2>/dev/null | head -1)
        echo "[ports] PG port $PG_PORT occupied${squatter:+ by container $squatter}, trying $(( PG_PORT + 1 ))" >&2
        PG_PORT=$(( PG_PORT + 1 ))
        pg_steps=$(( pg_steps + 1 ))
        if [ "$pg_steps" -gt 1000 ]; then
            echo "ERROR: could not find a free PostgreSQL port near $(( PG_PORT - pg_steps ))" >&2
            return 1
        fi
    done
    export PG_PORT
    export LUCIDOS_PG_PORT="$PG_PORT"

    echo "Starting PostgreSQL for workspace: $WORKSPACE (port $PG_PORT, container lucidos-pg-$PG_NAME)"

    docker rm -f "lucidos-pg-$PG_NAME" 2>/dev/null || true

    if ! docker compose -p "lucidos-$PG_NAME" -f "$PROJECT_DIR/docker-compose.dev.yml" up -d 2>&1; then
        local squatter
        squatter=$(docker ps --filter "publish=$PG_PORT" --format "{{.Names}}" 2>/dev/null | head -1)
        if [ -n "$squatter" ]; then
            echo "ERROR: port $PG_PORT is already bound by container '$squatter'" >&2
            echo "Stop it first: docker stop $squatter" >&2
        else
            local pid
            pid=$(lsof -ti :"$PG_PORT" -sTCP:LISTEN 2>/dev/null | head -1)
            if [ -n "$pid" ]; then
                local cmd
                cmd=$(ps -p "$pid" -o command= 2>/dev/null | head -c 80)
                echo "ERROR: port $PG_PORT is already bound by pid $pid ($cmd)" >&2
            else
                echo "ERROR: docker compose up failed (see output above)" >&2
            fi
        fi
        return 1
    fi

    echo -n "Waiting for PostgreSQL"
    for _ in {1..30}; do
        if docker exec "lucidos-pg-$PG_NAME" pg_isready -U lucidos > /dev/null 2>&1; then
            echo " ready!"
            break
        fi
        echo -n "."
        sleep 1
    done

    # Finish a pending PG 17 → 18 migration (restore into the fresh cluster)
    # before anything else touches the database. A failed restore aborts the
    # start so the engine never boots on a half-restored event store.
    _restore_postgres_major_dump_if_pending || return 1

    docker exec "lucidos-pg-$PG_NAME" psql -U lucidos -d lucidos -c "CREATE EXTENSION IF NOT EXISTS vector;" > /dev/null 2>&1 || true
}

# ── pid_is_live ─────────────────────────────────────────────────────────
# True if pid $1 is a process that is actually still running. A ZOMBIE (state
# `Z`, `<defunct>`) is NOT: it has already exited and only lingers in the
# process table until its parent reaps it.
#
# This exists because `kill -0 <pid>` SUCCEEDS for a zombie, so a bare signal
# probe reports a dead engine as running. On 2026-07-31 a workspace engine had
# been a defunct child of the gateway for a day, `status.sh --json` still said
# `engine_running: true`, and the workspace switcher rendered a healthy dot that
# sent an iOS PWA to a port nothing was listening on.
#
# `ps -o state=` is the portable answer (macOS and Linux both print a single
# state letter, `Z` for a zombie) and it also covers the pid-does-not-exist
# case, where `ps` exits non-zero and prints nothing. A non-numeric or empty
# pid (a truncated / garbage pidfile) is never live.
pid_is_live() {
    local pid="$1" state
    [ -n "$pid" ] || return 1
    case "$pid" in
        *[!0-9]*) return 1 ;;
    esac
    state="$(ps -o state= -p "$pid" 2>/dev/null)" || return 1
    # Strip the padding `ps` adds around the state column before matching.
    state="${state//[[:space:]]/}"
    [ -n "$state" ] || return 1
    case "$state" in
        Z*) return 1 ;;
    esac
    return 0
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
        done < <(pgrep -f "(dev|web-dev|tauri-dev)\\.sh.*$WORKSPACE" 2>/dev/null || true)
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
        if [ -f "$ENGINE_PIDFILE" ]; then
            local old_pid
            old_pid="$(cat "$ENGINE_PIDFILE" 2>/dev/null || true)"
            if [ -n "$old_pid" ] && kill -0 "$old_pid" 2>/dev/null; then
                echo "Stopping existing engine for rebuild (PID $old_pid)..."
                kill -USR1 "$old_pid" 2>/dev/null || true
                engine_pids_to_reap="$old_pid"
                killed=1
            fi
            rm -f "$ENGINE_PIDFILE"
        fi

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

    # Release this workspace's frontend marker (skip in --engine-only mode). In
    # built mode the marker is the SHARED build-watch pid (the engine serves
    # dist/ directly, no per-workspace preview) — release_frontend_marker removes
    # the file without killing the shared watch; a distinct dev-server pid (e2e)
    # is killed.
    if [ -z "$ENGINE_ONLY" ]; then
        [ -n "$(release_frontend_marker "$FRONTEND_PIDFILE")" ] && killed=1
    fi

    # The --built mode `vite build --watch` is now a checkout-level singleton
    # shared across workspaces (build_watch_pidfile), so a per-workspace restart
    # must NOT tear it down — start_frontend_built reuses it when healthy and
    # rebuilds only when it's dead, dist/ is broken, or this is a SOLO `-b`.
    # Clean up a legacy per-workspace build-watch from pre-singleton runs so it
    # can't linger as an orphan duplicating the shared one.
    if [ -z "$ENGINE_ONLY" ] && [ -f "$WORKSPACE/.lucidos/build-watch.pid" ]; then
        local legacy_bw
        legacy_bw="$(cat "$WORKSPACE/.lucidos/build-watch.pid" 2>/dev/null || true)"
        if [ -n "$legacy_bw" ] && kill -0 "$legacy_bw" 2>/dev/null; then
            echo "Stopping legacy per-workspace build-watch (PID $legacy_bw)..."
            kill "$legacy_bw" 2>/dev/null || true
            killed=1
        fi
        rm -f "$WORKSPACE/.lucidos/build-watch.pid"
    fi

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

# ── select_cargo_lock_holders ───────────────────────────────────────────
# Print the PIDs of real `cargo` processes whose command line includes
# `check` — the IDE / rust-analyzer processes that hold the shared `target/`
# build lock and must be cleared before a fresh `cargo build`.
#
# CRITICAL — filter by the process's EXECUTABLE, not a substring of its whole
# command line. `pgrep -f 'cargo check'` matches the phrase ANYWHERE in a
# process's argv, which also snares coding-agent subprocesses (claude / codex)
# whose injected prompt or args merely CONTAIN "cargo check" (a CC session
# working on a build does). Killing those by PID bypasses their process-group
# isolation and SIGTERMs a live coding-agent session — in THIS workspace or,
# because `target/` is shared across workspaces launched from one checkout, in
# ANOTHER workspace entirely. That is the exit=143 cross-workspace kill that
# silently terminated a parked CC session during an unrelated workspace's
# rebuild. Matching on the executable basename (`cargo`) keeps the
# lock-release intent while making it impossible to target a CC subprocess.
select_cargo_lock_holders() {
    local p comm
    for p in $(pgrep -f 'cargo check' 2>/dev/null || true); do
        # `ps -o comm=` is the executable path (macOS) or a bare name; compare
        # the basename so a rustup/homebrew `cargo` shim still matches and a
        # `claude` / `node` / `codex` subprocess never does.
        comm="$(ps -p "$p" -o comm= 2>/dev/null || true)"
        [ "${comm##*/}" = "cargo" ] && printf '%s\n' "$p"
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

# `release` when the caller asked for a release build, else `debug`.
engine_build_profile() {
    if [ -n "${RELEASE:-}" ]; then echo "release"; else echo "debug"; fi
}

# Directory the launch binaries for a (profile, variant) pair are published to.
#
# Deliberately NOT under `target/`, and that is the whole point of the directory
# (ADR 0022 originally put it at `target/<profile>/launch/<variant>`; ADR 0063
# amending that records why). `cargo clean` removes `target/` wholesale, and the
# launch dir is where the running system's `lucidos` CLI lives: the engine finds
# it by walking up from its own exe (`find_lucidos_cli_dir`) and prepends that
# dir to PATH for every spawned coding-agent session and trigger subprocess. So
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
# but the engine SIGKILLs the whole build process group when a second Apply
# supersedes a build (`engine_version::BuildProcessGroupGuard`), and no trap
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

# Classify a published binary's baked build id against the checkout's current
# HEAD: `current`, `stale`, or `unknown`.
#
# `stale` means the build did NOT produce a binary for the source that is on
# disk now — `build.rs` stamps the id when the build script RUNS, so a build
# that starts at commit N and finishes after an Apply moved main to N+1
# publishes an N binary. `unknown` (no git, unreadable/empty id, a no-git
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
    local slot="$PROJECT_DIR/scripts/with-build-slot.sh"
    if [ -n "${RELEASE:-}" ]; then
        "$slot" --label "engine build (release)" -- \
            cargo build --locked -p lucidos-engine "${feature_args[@]}" -p lucidos-gateway -p lucidos-cli --release
    else
        "$slot" --label "engine build" -- \
            cargo build --locked -p lucidos-engine "${feature_args[@]}" -p lucidos-gateway -p lucidos-cli
    fi
}

# Locate already-built binaries for the no-build path. Prefers the published
# launch dir; falls back to cargo's shared uplift path with a warning (a fresh
# checkout, or a hand-run `cargo build`, has no published dir yet). Profile
# order mirrors the historical behavior: a release request falls back to debug,
# a debug request does not reach for release. Sets ENGINE_BIN + GATEWAY_BIN as a
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
    if [ -n "${RELEASE:-}" ]; then
        dirs+=("$(launch_bin_dir release "$variant")" "$PROJECT_DIR/target/release")
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

    local launch_dir uplift_dir
    launch_dir="$(launch_bin_dir)"
    uplift_dir="$PROJECT_DIR/target/$(engine_build_profile)"

    # Clear IDE/rust-analyzer `cargo check` processes holding the shared
    # target/ build lock. Scoped to real cargo processes — see
    # select_cargo_lock_holders for why a raw `pgrep -f` is unsafe.
    local check_pids
    check_pids=$(select_cargo_lock_holders)
    if [ -n "$check_pids" ]; then
        echo "Killing cargo check processes to release build lock..."
        echo "$check_pids" | xargs kill 2>/dev/null || true
    fi

    # Remove stale lock files (can linger after sleep/wake with no holding process)
    rm -f "$PROJECT_DIR/target/.cargo-lock" "$PROJECT_DIR/target/debug/.cargo-lock" "$PROJECT_DIR/target/release/.cargo-lock" "$PROJECT_DIR/target/.package-cache"

    echo ""
    echo "Building engine..."
    run_engine_cargo_build
    publish_launch_binaries "$uplift_dir" "$launch_dir" || true

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
        run_engine_cargo_build
        publish_launch_binaries "$uplift_dir" "$launch_dir" || true
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
#   • GATEWAY_PORT = a FIXED machine-global port (default 5251 in dev, the
#     gateway's own DEFAULT_GATEWAY_PORT; override with LUCIDOS_DEV_GATEWAY_PORT;
#     the packaged desktop app keeps the historical 5252, so dev + packaged
#     coexist out of the box) — NOT the
#     per-workspace API_PORT. There is ONE shared gateway per machine; it binds
#     this fixed port and serves `/<slug>/` (proxying to each engine) + the
#     picker at `/~/`. A fixed port is required because every workspace launch
#     reuses the SAME gateway (ADR 0014 §10) rather than starting its own.
# Both are reachable at once in dev; the engine serves the built dist/ directly
# via LUCIDOS_STATIC_DIR (no Vite in the serving path). Writes .lucidos/ports
# (the engine's direct port, so the CLI reaches the engine). Exports env vars.
swap_ports() {
    ENGINE_PORT="$VITE_PORT"
    GATEWAY_PORT="${LUCIDOS_DEV_GATEWAY_PORT:-5251}"

    # The ports file records the engine's direct port — the CLI / cross-workspace
    # callers talk to the engine, not the gateway.
    cat > "$WORKSPACE/.lucidos/ports" <<EOF
API_PORT=$ENGINE_PORT
VITE_PORT=$ENGINE_PORT
PG_PORT=$PG_PORT
PG_DATABASE=$(workspace_database_name)
DATABASE_URL=$(workspace_database_url)
EOF

    # That truncated the file detect_tls appended PROTO to, so write it again.
    # Every reader falls back to https when the line is missing. On a machine
    # with no dev certs the engine then serves plain http, and each caller
    # fails at the TLS handshake with "record overflow".
    if [ -n "${PROTO:-}" ]; then
        echo "PROTO=$PROTO" >> "$WORKSPACE/.lucidos/ports"
    fi

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

# ── start_caffeinate ────────────────────────────────────────────────────
# Prevent idle/disk sleep while this script runs (-w $$).
# Clamshell sleep is handled separately by pmset above.
start_caffeinate() {
    [ "$(uname)" = "Darwin" ] || return 0   # macOS-only (caffeinate); no-op on Linux/CI
    # `-w $$` ties it to this shell's lifetime, so there is no pid to track.
    caffeinate -im -w $$ &
    enable_clamshell_prevention
}

# ── Network bind (dev) ───────────────────────────────────────────────────
# The engine and the gateway both default to loopback. Only the GATEWAY opts
# back into the network here: it authenticates every caller (ADR 0094), so it is
# the one process that may face one. A directly-launched engine authenticates
# nobody, so it stays where the engine's own resolver puts it.
# ~/.lucidos/network.toml owns the address when it exists. The shell only
# chooses between pinning a bind and deferring to that resolver.
network_toml_exists() {
    [ -f "$HOME/.lucidos/network.toml" ]
}

# Bind for a directly-launched engine (start_engine): legacy no-gateway dev,
# tauri-dev, and e2e. Nothing authenticates this port, so widening it is a
# deliberate act by the developer, never a launch-script default.
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

# ── start_engine ────────────────────────────────────────────────────────
# Run engine in background with caffeinate, write PID, wait for health (30s).
# Reuses an existing healthy engine if one is already running for this workspace.
# Sets ENGINE_PID.
start_engine() {
    # Direct-front launch (legacy no-gateway dev, tauri-dev, e2e): the engine IS
    # the user-facing door on its own port, and nothing authenticates it. So it
    # stays on loopback unless the developer widens it deliberately. The gateway
    # path does NOT use this function. It spawns engines itself with the right
    # bind (see gateway stack.rs spawn_engine).
    apply_dev_engine_bind

    # Check if an existing engine is already healthy on our port
    if [ -f "$ENGINE_PIDFILE" ]; then
        local existing_pid
        existing_pid="$(cat "$ENGINE_PIDFILE" 2>/dev/null || true)"
        if [ -n "$existing_pid" ] && kill -0 "$existing_pid" 2>/dev/null; then
            if curl -sk "$PROTO://localhost:$ENGINE_PORT/api/v1/health" >/dev/null 2>&1; then
                echo ""
                echo "Reusing existing engine (PID $existing_pid) on port $ENGINE_PORT"
                ENGINE_PID="$existing_pid"
                start_caffeinate
                return
            fi
        fi
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

    start_caffeinate

    # Spawn the supervisor (engine_supervisor.sh:run_supervised) as a
    # backgrounded subshell. It loops the engine binary so an unexpected
    # kill (SIGKILL from a stale worktree's ports.sh, OOM, panic) becomes
    # a 1–30 s blip instead of a session outage. Legit stops (exit 0 /
    # 130 / 138 from SIGUSR1/SIGINT, or SIGTERM to the supervisor itself
    # from kill_stale_processes' `pkill -P`) flow through cleanly.
    ( run_supervised "$ENGINE_PIDFILE" "$ENGINE_LOG" "$ENGINE_BIN" ) &
    ENGINE_SUPERVISOR_PID=$!

    # Wait for the supervisor to write the engine pid. The supervisor
    # rewrites the pidfile on every (re)start, so this also picks up a
    # within-startup restart (engine crashes immediately, supervisor
    # respawns it).
    local pid_deadline=$(( $(date +%s) + 5 ))
    while [ "$(date +%s)" -lt "$pid_deadline" ]; do
        if [ -s "$ENGINE_PIDFILE" ]; then
            ENGINE_PID="$(cat "$ENGINE_PIDFILE" 2>/dev/null || true)"
            [ -n "$ENGINE_PID" ] && kill -0 "$ENGINE_PID" 2>/dev/null && break
        fi
        sleep 0.1
    done
    if [ -z "${ENGINE_PID:-}" ] || ! kill -0 "$ENGINE_PID" 2>/dev/null; then
        echo ""
        echo "ERROR: Engine supervisor did not start the engine within 5s. Check logs:"
        tail -10 "$ENGINE_LOG"
        kill -KILL "$ENGINE_SUPERVISOR_PID" 2>/dev/null || true
        exit 1
    fi

    # Wait for engine to be ready. Cold boot does pgvector init, migrations and
    # the memory index load, which can push past 30s on a fresh workspace or a
    # slow disk. Give it 90s before declaring failure. (The embedding model is
    # NOT in that window: it loads in the background and boot never waits on
    # it.)
    # Re-read $ENGINE_PIDFILE each tick so a supervisor restart during
    # startup updates ENGINE_PID rather than failing the kill -0 check.
    echo -n "Waiting for engine"
    local engine_ready=""
    for _ in {1..90}; do
        ENGINE_PID="$(cat "$ENGINE_PIDFILE" 2>/dev/null || true)"
        if [ -z "$ENGINE_PID" ] || ! kill -0 "$ENGINE_PID" 2>/dev/null; then
            # Supervisor is between restarts; wait one tick for the next
            # spawn before checking health.
            echo -n "."
            sleep 1
            continue
        fi
        if curl -sk "$PROTO://localhost:$ENGINE_PORT/api/v1/health" >/dev/null 2>&1; then
            echo " ready!"
            engine_ready="yes"
            break
        fi
        echo -n "."
        sleep 1
    done

    if [ -z "$engine_ready" ]; then
        echo ""
        echo "ERROR: Engine failed to start within 90 seconds. Check logs:"
        tail -20 "$ENGINE_LOG"
        kill -KILL "$ENGINE_SUPERVISOR_PID" 2>/dev/null || true
        exit 1
    fi
}

# ── Workspace gateway (ADR 0014) ──────────────────────────────────────────
# Dev runs ONE machine-global `lucidos-gateway` binary as the user-facing front
# (on the fixed gateway port, default 5251 in dev — the packaged app keeps 5252).
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
# is keyed off $HOME, not $WORKSPACE. Mirrors the gateway's own resolve_app_data
# default ($HOME/.lucidos/gateway).
gateway_data_dir() { echo "${LUCIDOS_GATEWAY_DATA:-$HOME/.lucidos/gateway}"; }
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
    local token_file="$HOME/.lucidos/local-token"
    if [ -r "$token_file" ]; then
        curl -H "x-lucidos-local-token: $(cat "$token_file")" "$@"
    else
        curl "$@"
    fi
}

# Stable, filesystem/URL-safe slug from the workspace dir basename — the routing
# key (/<slug>/). Mirrors gateway::registry::slugify; empty → "workspace".
workspace_slug() {
    local s
    s="$(basename "$WORKSPACE" | tr '[:upper:]' '[:lower:]' | sed -E 's/[^a-z0-9]+/-/g; s/^-+//; s/-+$//')"
    [ -n "$s" ] && echo "$s" || echo "workspace"
}

# Seed/refresh this workspace's entry in the SHARED gateway registry, preserving
# every other workspace's entry AND this workspace's user-set display name +
# autostart toggle (so a picker rename / autostart flip sticks across relaunch —
# only the runtime fields dir/port are refreshed). A brand-new entry
# is appended with autostart OFF (manual): an explicit launch starts it this
# session, but it won't auto-start on a future gateway boot unless the user opts
# in via the picker toggle. The registry `port` is the engine's own port
# (ENGINE_PORT = VITE_PORT, ADR 0014 §4): the gateway spawns the engine on
# loopback there and proxies `/<slug>/` to it. Sets GATEWAY_WS_ID.
seed_gateway_registry() {
    local data reg id name
    data="$(gateway_data_dir)"
    mkdir -p "$data/config"
    reg="$data/config/workspaces.json"
    id="$(workspace_slug)"
    name="$(basename "$WORKSPACE")"
    GATEWAY_WS_ID="$id"
    python3 - "$reg" "$id" "$name" "$WORKSPACE" "$ENGINE_PORT" <<'PY'
import json, sys
reg, wid, name, wdir, port = sys.argv[1:6]
try:
    with open(reg) as f:
        data = json.load(f)
except Exception:
    data = {}
wss = data.get("workspaces", [])
found = False
for w in wss:
    if w.get("id") == wid:
        # Preserve user-set name + autostart; refresh only the runtime fields.
        w["dir"] = wdir
        w["port"] = int(port)
        # ADR 0014 §6/§7: steady state is one shared PG cluster with one
        # database per workspace. A legacy database_url meant "migrate from
        # per-workspace PG"; once web-dev has migrated/verified this workspace,
        # remove it so the gateway provisions/uses the shared DB.
        w.pop("database_url", None)
        w.setdefault("autostart", False)
        found = True
        break
if not found:
    wss.append({"id": wid, "name": name, "dir": wdir, "port": int(port),
                "autostart": False})
data["workspaces"] = wss
with open(reg, "w") as f:
    json.dump(data, f, indent=2)
PY
    echo "Gateway registry: $reg  (workspace '$id' → engine :$ENGINE_PORT, database $(workspace_database_name), shared gateway :$GATEWAY_PORT)"
}

# Wait for /<slug>/api/v1/health — the engine the gateway spawned.
wait_for_workspace_health() {
    echo -n "Waiting for workspace '$GATEWAY_WS_ID' engine"
    for _ in $(seq 1 90); do
        if curl -sk "$PROTO://localhost:$GATEWAY_PORT/$GATEWAY_WS_ID/api/v1/health" >/dev/null 2>&1; then
            echo " ready!"; return 0
        fi
        echo -n "."; sleep 1
    done
    echo ""
    echo "WARNING: workspace engine not healthy yet — check the picker or the gateway log:"
    echo "  $(gateway_log)"
}

# Start (or reuse) the ONE shared workspace gateway on the fixed GATEWAY_PORT. It
# spawns + supervises each workspace's engine network-bound on its own
# ENGINE_PORT (so the app is reachable directly there too, ADR 0014 §4) and
# routes /<slug>/ + the picker at /~/. Because new workspaces default to
# autostart OFF, the gateway's own boot does NOT spawn this workspace — so AFTER
# the gateway is up we always POST its control API to start (or respawn, for an
# Apply) THIS workspace's engine. Sets GATEWAY_PID; leaves ENGINE_SUPERVISOR_PID
# EMPTY (the detached gateway supervisor is a machine-global daemon web-dev.sh
# must never `wait` on — its non-tty wait falls back to the pidfile poll).
start_gateway() {
    GATEWAY_MODE=1
    # GATEWAY_PORT (fixed, default 5251 in dev) and ENGINE_PORT (=VITE_PORT) are set by
    # swap_ports. The gateway's data dir / pidfile / log are machine-global.
    local gw_pidfile gw_log
    mkdir -p "$(gateway_data_dir)"
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

    # Reuse a healthy gateway already on the port (no -b restart). Ask it to
    # (re)start this workspace's stack so a rebuilt binary / refreshed registry
    # takes effect — this is the engine-only Apply path in gateway dev. The
    # gateway's own surface lives behind the sigil namespace (/~/, ADR 0014 §2).
    if [ -f "$gw_pidfile" ]; then
        local existing; existing="$(cat "$gw_pidfile" 2>/dev/null || true)"
        if [ -n "$existing" ] && kill -0 "$existing" 2>/dev/null \
           && curl -sk "$PROTO://localhost:$GATEWAY_PORT/~/api/v1/health" >/dev/null 2>&1; then
            echo "Reusing existing gateway (PID $existing) on port $GATEWAY_PORT"
            GATEWAY_PID="$existing"; ENGINE_SUPERVISOR_PID=""
            start_caffeinate
            gateway_curl -sk -X POST "$PROTO://localhost:$GATEWAY_PORT/~/api/v1/control/workspaces/$GATEWAY_WS_ID/restart" >/dev/null 2>&1 || true
            wait_for_workspace_health
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
    start_caffeinate

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
            local _i; for _i in $(seq 1 10); do port_is_free "$GATEWAY_PORT" && break; sleep 0.3; done
            port_is_free "$GATEWAY_PORT" || kill -KILL "$occ" 2>/dev/null || true
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
    ( run_gateway_supervised "$gw_pidfile" "$gw_log" "$GATEWAY_BIN" ) &
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
    # autostart workspaces, but NOT this just-launched one (autostart defaults
    # OFF), so start it explicitly via the control API — same call the reuse path
    # makes, so both paths end with this workspace's engine running.
    gateway_curl -sk -X POST "$PROTO://localhost:$GATEWAY_PORT/~/api/v1/control/workspaces/$GATEWAY_WS_ID/restart" >/dev/null 2>&1 || true
    wait_for_workspace_health
}

# The gateway is now ONE shared machine-global process fronting every workspace,
# so stop.sh's stop_workspace does NOT tear it down — it POSTs the gateway's
# /stop control API to stop just the target workspace's engine (the gateway drops
# that stack so its supervisor won't respawn it; the registry entry survives, so
# the workspace stays listed in the picker as stopped). web-dev.sh's Ctrl+C trap
# (cleanup_processes) likewise leaves the shared gateway running. To stop the
# gateway itself: `kill $(cat "$(gateway_data_dir)/gateway.pid")`.

# ── running_frontend_workspaces_in_project ─────────────────────────────
# Echo workspace names whose frontend.pid points to a live Vite process
# whose physical cwd is inside the given project directory. Comparison
# uses `pwd -P`, so a Vite running from one physical checkout (e.g. main)
# does NOT match a different physical checkout (e.g. a CC worktree under
# .lucidos/worktrees/) — installs in one don't corrupt the other's
# node_modules. Assumes Vite was spawned with cwd inside its project root
# (see start_vite); a Vite launched with `--root` from elsewhere would
# slip past this check.
# Subshell so `nullglob` doesn't leak into the caller's shell options.
running_frontend_workspaces_in_project() (
    shopt -s nullglob
    local project="$1"
    # Optional pid to ignore. In BUILT mode `start_frontend_built` records the
    # shared build-watch pid as every workspace's `frontend.pid`, so the marker
    # alone cannot tell a Vite dev server from the watcher itself. A caller
    # asking "is a dev server holding node_modules" passes the build-watch pid
    # and gets the honest answer; every existing caller passes nothing and keeps
    # the ref-count semantics `teardown_shared_build_watch_if_idle` needs.
    local exclude_pid="${2:-}"
    local project_real
    project_real="$(cd "$project" 2>/dev/null && pwd -P || true)"
    [ -n "$project_real" ] || return 0
    local pidfile
    for pidfile in "$HOME"/workspaces/*/.lucidos/frontend.pid; do
        local pid ws_dir vite_cwd vite_real
        # `cat … || true` keeps a transient unreadable file from killing the
        # subshell under the caller's `set -e`.
        pid="$(cat "$pidfile" 2>/dev/null || true)"
        [ -n "$pid" ] || continue
        # An `if`, not a bare `A && B && continue`: a caller running under
        # `set -e` aborts the whole subshell on the miss, which reads as "no
        # frontend is running" and is the wrong answer in the unsafe direction.
        if [ -n "$exclude_pid" ] && [ "$pid" = "$exclude_pid" ]; then continue; fi
        kill -0 "$pid" 2>/dev/null || continue
        # `lsof -p PID -a -d cwd -Fn` prints `n<path>` for the cwd FD entry.
        vite_cwd="$(lsof -p "$pid" -a -d cwd -Fn 2>/dev/null | awk '/^n/ {print substr($0,2); exit}')"
        [ -n "$vite_cwd" ] || continue
        vite_real="$(cd "$vite_cwd" 2>/dev/null && pwd -P || true)"
        [ -n "$vite_real" ] || continue
        # Conflict if vite's cwd is the project root or anywhere inside it.
        # Trailing slashes prevent `/a/b` from matching `/a/bb`.
        case "$vite_real/" in
            "$project_real/"|"$project_real"/*)
                ws_dir="${pidfile%/.lucidos/frontend.pid}"
                echo "${ws_dir##*/}"
                ;;
        esac
    done
)

# ── shared build-watch (checkout-level singleton) ───────────────────────
# `vite build --watch` produces the SHARED crates/lucidos-app/dist/ that every
# workspace of this checkout serves (each engine serves it directly via
# LUCIDOS_STATIC_DIR, ADR 0014). It is therefore a checkout-level singleton, NOT
# per workspace: the first workspace to start it owns it; later workspaces reuse
# it (start_frontend_built). Rebuilding it per workspace would republish a
# byte-fresh sw.js into the shared dist/ and spuriously fire the "New version
# available" toast on every OTHER workspace's open tab (the determinism guard
# means identical source never changes the BUILD_ID — so the toast only fires
# when a republish drags those tabs forward).
build_watch_pidfile() { echo "$PROJECT_DIR/crates/lucidos-app/.build-watch/pid"; }
build_watch_log()     { echo "$PROJECT_DIR/crates/lucidos-app/.build-watch/log"; }

# Tear down the shared build-watch only when NO workspace of this checkout is
# still serving the frontend. Call AFTER this workspace's frontend.pid has been
# removed, so running_frontend_workspaces_in_project no longer counts us. No-op
# when no shared build-watch is recorded.
teardown_shared_build_watch_if_idle() {
    local pidfile; pidfile="$(build_watch_pidfile)"
    [ -f "$pidfile" ] || return 0
    if [ -n "$(running_frontend_workspaces_in_project "$PROJECT_DIR")" ]; then
        return 0   # another workspace still serves this checkout — keep it alive
    fi
    local pid; pid="$(cat "$pidfile" 2>/dev/null || true)"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        echo "Stopping shared frontend build-watch (PID $pid) — no workspaces left serving this checkout"
        kill "$pid" 2>/dev/null || true
    fi
    rm -f "$pidfile"
}

# ── release_frontend_marker ─────────────────────────────────────────────
# Release a workspace's frontend marker (its `.lucidos/frontend.pid`). ADR 0014:
# the engine serves the built dist/ directly — there is no per-workspace `vite
# preview`. In built mode frontend.pid records the SHARED build-watch pid purely
# for ref-counting (running_frontend_workspaces_in_project), so it must NEVER be
# killed here (peers may share it) — only the FILE is removed; the shared watch's
# fate is decided by teardown_shared_build_watch_if_idle. A frontend.pid that
# differs from the shared build-watch pid is a genuine per-workspace process (the
# legacy/e2e live dev server) and IS killed. Echoes "1" if it killed a process.
release_frontend_marker() {
    local pidfile="$1"
    [ -f "$pidfile" ] || return 0
    local fpid bwpid
    fpid="$(cat "$pidfile" 2>/dev/null || true)"
    bwpid="$(cat "$(build_watch_pidfile)" 2>/dev/null || true)"
    if [ -n "$fpid" ] && [ "$fpid" != "$bwpid" ] && kill -0 "$fpid" 2>/dev/null; then
        # Human message to stderr so callers' `$(release_frontend_marker …)` capture
        # gets ONLY the "1" sentinel on stdout (and still surfaces the line).
        echo "Stopping frontend dev server (PID $fpid)" >&2
        kill "$fpid" 2>/dev/null || true
        rm -f "$pidfile"
        echo "1"
        return 0
    fi
    rm -f "$pidfile"
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
# every package.json in the tree (heavy/irrelevant dirs pruned) plus the
# root lockfile. Content-only by design — unlike an mtime comparison, a
# no-op rewrite (a git checkout, `git worktree add`, or a CC change apply
# rewrites package.json and bumps its mtime without changing a byte) yields
# an identical fingerprint, so it can't trigger a spurious reinstall that
# would deadlock an engine-only restart while a frontend is running.
# cksum is portable (macOS + Linux) and sufficient for change detection.
_deps_fingerprint() {
    local root="$1"
    {
        find "$root" \
            \( -name node_modules -o -name .git -o -name target \
               -o -name .lucidos -o -name dist \) -prune \
            -o -name package.json -print 2>/dev/null \
            | LC_ALL=C sort | while IFS= read -r f; do cat "$f"; done
        [ -f "$root/package-lock.json" ] && cat "$root/package-lock.json"
    } | cksum
}

# ── ensure_npm_deps ───────────────────────────────────────────────────
# Run npm install if node_modules is missing or the dependency fingerprint
# changed (see _deps_fingerprint — content, not mtime). Refuses if any other
# workspace has a running frontend dev server inside THIS project
# ($PROJECT_DIR) — npm workspaces hoists deps to one shared node_modules
# tree, so mutating it under a running Vite silently corrupts its in-memory
# module graph (stale inodes) and turns it into a wedged "200 with wrong
# body" server — manifesting as a blank page in the browser. Vites running
# from a different physical checkout (e.g. a CC worktree) don't share
# node_modules with this project and aren't blocked.
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
        local active
        active="$(running_frontend_workspaces_in_project "$PROJECT_DIR")"
        if [ -n "$active" ]; then
            # Two non-disruptive paths deliberately keep the running frontend
            # alive and so MUST NOT touch its shared node_modules:
            #   • --engine-only (CC Apply switch): kill_stale_processes skips the
            #     Vite + build-watch teardown when ENGINE_ONLY is set, so the
            #     restart never killed it.
            #   • --engine-build (Apply background rebuild): just compiles the new
            #     on-disk binary and exits — no restart, no teardown at all.
            # For BOTH, build_sdk runs (before the ENGINE_ONLY early-exit in
            # web-dev.sh, and unconditionally on the --engine-build path), so a
            # hard-fail (exit 1) here kills the whole build/restart: an engine-only
            # restart would leave the workspace with NO engine, and a background
            # rebuild would report "New engine version failed to build" even though
            # the engine binary compiled fine. So we skip the install, keep the
            # existing (working) node_modules, and let it proceed; the deferred deps
            # land on the next full frontend restart (the stamp is intentionally
            # left un-updated so that restart re-detects the change). Outside these
            # two paths a running frontend IS a genuine conflict — a second
            # workspace launch against shared deps — so hard-fail and say so.
            if [ -n "${ENGINE_ONLY:-}" ] || [ -n "${ENGINE_BUILD_ONLY:-}" ]; then
                echo "" >&2
                echo "WARNING: $label changed ($needs_install) but a frontend in this checkout is running for:" >&2
                while IFS= read -r ws; do
                    [ -n "$ws" ] && echo "  - $ws" >&2
                done <<< "$active"
                local skip_ctx="engine restart"
                [ -n "${ENGINE_BUILD_ONLY:-}" ] && skip_ctx="background build"
                echo "Skipping install to keep that frontend alive; the $skip_ctx will still proceed." >&2
                echo "Run a full restart (./scripts/stop.sh -w <name> then ./scripts/web-dev.sh -w <name>) to pick up the new dependencies." >&2
                echo "" >&2
                return 0
            fi
            echo "" >&2
            echo "ERROR: $label install needed ($needs_install), but a frontend in this checkout is running for:" >&2
            while IFS= read -r ws; do
                [ -n "$ws" ] && echo "  - $ws" >&2
            done <<< "$active"
            echo "" >&2
            echo "Installing now would silently corrupt those Vite processes (blank page)." >&2
            echo "Stop them first:  ./scripts/stop.sh -w <name>" >&2
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

# ── build_sdk ──────────────────────────────────────────────────────────
# Build the Lucidos SDK bundle (packages/lucidos-sdk → dist/sdk.js).
# The engine serves this at /api/v1/sdk.js for app UIs.
build_sdk() {
    # SDK deps are hoisted to root node_modules by npm workspaces
    ensure_npm_deps "$PROJECT_DIR" "workspace dependencies"
    echo "Building Lucidos SDK..."
    (cd "$PROJECT_DIR/packages/lucidos-sdk" && npm run build)
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
# picks up new source), and a fresh `vite build` here is sub-second. Writes NO
# frontend.pid — there is no process to ref-count, so release_frontend_marker /
# teardown_shared_build_watch_if_idle stay no-ops for this path (they no-op when
# the marker files are absent).
build_frontend_oneshot() {
    echo "Building frontend (one-shot vite build)..."
    # Drop the atomic-publish scratch dirs a prior `vite build --watch` may have
    # left in this checkout; a plain `vite build` (no LUCIDOS_ATOMIC_DIST) empties
    # and writes dist/ itself, so dist/ is left for vite to manage.
    rm -rf "$FRONTEND_DIR/dist.staging" "$FRONTEND_DIR/dist.prev"
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
# of this checkout (build_watch_pidfile). This launch REUSES a healthy shared
# build-watch instead of rebuilding when another workspace is already serving the
# checkout — a rebuild would republish a byte-fresh sw.js into the shared dist/ and
# spuriously fire "New version available" on those workspaces' open tabs. A SOLO
# `-b` restart still rebuilds from scratch (matches the pre-singleton behavior).
# (The build-watch can no longer serve stale CSS: each rebuild is a fresh `vite
# build` process with no incremental cache — so there is no wedge to remedy.)
start_frontend_built() {
    local bw_pidfile bw_log
    bw_pidfile="$(build_watch_pidfile)"
    bw_log="$(build_watch_log)"
    mkdir -p "${bw_pidfile%/*}"

    local existing_pid="" healthy=""
    if [ -f "$bw_pidfile" ]; then existing_pid="$(cat "$bw_pidfile" 2>/dev/null || true)"; fi
    if [ -n "$existing_pid" ] && kill -0 "$existing_pid" 2>/dev/null && [ -f "$FRONTEND_DIR/dist/index.html" ]; then
        healthy=1
    fi

    # No age-based recycle needed any more: the build-watch (dev-build-watch.mjs)
    # runs a CLEAN `vite build` in a fresh child process per change, so it has no
    # long-lived incremental cache to wedge. The served dist/ always reflects the
    # current source no matter how long the watch has run — the old "recycle an
    # over-age watch to clear a stale-CSS wedge" heuristic is obsolete.

    # Is another workspace of THIS checkout currently serving the shared dist/?
    # (This workspace's own frontend.pid was removed by kill_stale_processes, so
    # it is not counted here.)
    local others_serving
    others_serving="$(running_frontend_workspaces_in_project "$PROJECT_DIR")"

    # Reuse the healthy shared build-watch when another workspace is serving the
    # checkout (don't disturb their tabs) OR this isn't an explicit `-b` rebuild
    # (the watch already keeps dist/ current). Otherwise (re)build and take
    # ownership — covers a dead/broken watch and the solo `-b` rebuild.
    if [ -n "$healthy" ] && { [ -n "$others_serving" ] || [ -z "${BUILD:-}" ]; }; then
        echo "Reusing shared frontend build-watch (PID $existing_pid) serving $FRONTEND_DIR/dist."
        BUILD_WATCH_PID="$existing_pid"
    else
        if [ -n "$existing_pid" ] && kill -0 "$existing_pid" 2>/dev/null; then
            echo "Replacing existing frontend build-watch (PID $existing_pid)..."
            kill "$existing_pid" 2>/dev/null || true
        fi
        echo "Building frontend (fresh vite build per change)..."
        # Clean slate. dist/ is the LIVE dir the engine serves; dist.staging/dist.prev
        # are the atomic-publish scratch dirs (LUCIDOS_ATOMIC_DIST in vite.config.ts):
        # each build builds into dist.staging and renames it onto dist/ only after a
        # complete build, so a failed/interrupted rebuild can't leave the engine
        # serving a shell-less dist/ (the "404 on every page" failure mode).
        rm -rf "$FRONTEND_DIR/dist" "$FRONTEND_DIR/dist.staging" "$FRONTEND_DIR/dist.prev"
        # dev-build-watch.mjs runs a clean `vite build` (fresh process, no
        # incremental cache → no stale-CSS wedge) on every change, setting
        # LUCIDOS_ATOMIC_DIST for each child build itself. Output goes to a log
        # (not /dev/null) so a build failure is one `tail` away. `exec` makes the
        # tracked pid the node watcher itself, so teardown's `kill` lands on it
        # (firing its SIGTERM handler → kills the in-flight build, no orphan).
        # LUCIDOS_CLI_BIN lets the watcher raise a notification when a build
        # fails, which is the only way a wedged build is visible before somebody
        # reads the log. Absolute path, the same convention `desktop.rs`
        # `spawn_gateway` uses. LUCIDOS_WORKSPACE is already exported by
        # `resolve_workspace`, and the CLI needs both.
        local cli_bin="${ENGINE_BIN:+${ENGINE_BIN%/*}/lucidos}"
        (cd "$FRONTEND_DIR" && LUCIDOS_CLI_BIN="$cli_bin" exec node dev-build-watch.mjs) > "$bw_log" 2>&1 &
        BUILD_WATCH_PID=$!
        echo "$BUILD_WATCH_PID" > "$bw_pidfile"

        echo -n "Waiting for initial frontend build (log: $bw_log)"
        local build_deadline=$((SECONDS + 180))
        while (( SECONDS < build_deadline )); do
            if [ -f "$FRONTEND_DIR/dist/index.html" ]; then echo " done!"; break; fi
            if ! kill -0 "$BUILD_WATCH_PID" 2>/dev/null; then
                echo ""
                echo "ERROR: frontend build-watch exited before producing dist/index.html. See $bw_log" >&2
                rm -f "$bw_pidfile"
                exit 1
            fi
            echo -n "."
            sleep 1
        done
        if [ ! -f "$FRONTEND_DIR/dist/index.html" ]; then
            echo ""
            echo "ERROR: initial frontend build did not complete within 180s. See $bw_log" >&2
            kill "$BUILD_WATCH_PID" 2>/dev/null || true
            rm -f "$bw_pidfile"
            exit 1
        fi
    fi

    # No `vite preview` (ADR 0014): the engine serves dist/ directly via
    # LUCIDOS_STATIC_DIR. Record this workspace's "serving" marker as the SHARED
    # build-watch pid so teardown ref-counting (running_frontend_workspaces_in_project
    # → teardown_shared_build_watch_if_idle) keeps the watch alive while any
    # workspace of the checkout is up, and tears it down when the last one stops.
    # release_frontend_marker never kills this pid (it matches the build-watch).
    FRONTEND_PID="$BUILD_WATCH_PID"
    echo "$FRONTEND_PID" > "$FRONTEND_PIDFILE"
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
    # serving path) in every mode, including tauri-dev (the window loads dist/
    # from the engine via devUrl). An --engine-only restart leaves the running
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
    # Release this workspace's frontend marker (never kills the shared build-watch
    # — see release_frontend_marker; ADR 0014).
    release_frontend_marker "$FRONTEND_PIDFILE" >/dev/null
    # The `vite build --watch` is a checkout-level singleton shared across
    # workspaces; tear it down only when no workspace is still serving this
    # checkout's frontend (this workspace's frontend.pid was removed just above,
    # so it no longer counts).
    teardown_shared_build_watch_if_idle
    echo "Engine still running for workspace: $WORKSPACE (port $ENGINE_PORT)"
    echo "Stop with: ./scripts/stop.sh -w $WORKSPACE"
    echo "Shared PostgreSQL still running (container $(shared_pg_container)) for all workspaces."
    if _legacy_postgres_exists; then
        echo "Legacy per-workspace PostgreSQL is preserved for rollback. Decommission after verifying with:"
        echo "  ./scripts/decommission-legacy-postgres.sh -w $WORKSPACE"
    fi
}
