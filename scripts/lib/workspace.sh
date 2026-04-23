#!/bin/bash
# Shared functions for CognOS dev scripts (web-dev.sh, tauri-dev.sh).
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
#   SCRIPT_DIR, PROJECT_DIR, FRONTEND_DIR
# Globals set by callers:
#   SCRIPT_NAME  — basename of the calling script (for usage messages)

# ── parse_dev_args ──────────────────────────────────────────────────────
# Parse -w, -b, -r, -h flags. Sets WORKSPACE, BUILD, RELEASE.
parse_dev_args() {
    WORKSPACE="${COGNOS_WORKSPACE:-}"
    BUILD=""
    RELEASE=""
    ENGINE_ONLY=""
    FOLLOW_LOG=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            -w|--workspace) WORKSPACE="$2"; shift 2 ;;
            -b|--build) BUILD="1"; shift ;;
            -r|--release) RELEASE="1"; shift ;;
            -f|--follow) FOLLOW_LOG="1"; shift ;;
            --engine-only) ENGINE_ONLY="1"; BUILD="1"; shift ;;
            -h|--help)
                echo "Usage: $SCRIPT_NAME -w <workspace> [OPTIONS]"
                echo ""
                echo "Options:"
                echo "  -w, --workspace DIR   Workspace directory or name (required)"
                echo "  -b, --build           Build engine before starting"
                echo "  -r, --release         Build in release mode (slower build, faster runtime)"
                echo "  -f, --follow          Tail the engine log after startup (default: exit after ready)"
                echo "  --engine-only         Rebuild and restart only the engine (skip Vite, keep parent scripts)"
                echo "  -h, --help            Show this help"
                echo ""
                echo "Examples:"
                echo "  $SCRIPT_NAME -w dev               # ~/workspaces/dev"
                echo "  $SCRIPT_NAME -w personal -b       # ~/workspaces/personal, build first"
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
        echo "  $SCRIPT_NAME -w ~/workspaces/personal"
        exit 1
    fi
}

# ── resolve_workspace ───────────────────────────────────────────────────
# Expand bare names to ~/workspaces/<name>, create dirs, resolve to absolute.
# Sets: WORKSPACE, ENGINE_PIDFILE, FRONTEND_PIDFILE, ENGINE_LOG, PG_NAME
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

    # Ensure workspace directories exist
    mkdir -p "$WORKSPACE/artifacts"
    mkdir -p "$WORKSPACE/data/postgres"
    mkdir -p "$WORKSPACE/.cognos"

    # Workspace-scoped state files
    ENGINE_PIDFILE="$WORKSPACE/.cognos/engine.pid"
    FRONTEND_PIDFILE="$WORKSPACE/.cognos/frontend.pid"
    ENGINE_LOG="$WORKSPACE/.cognos/engine.log"

    # Compute a short name for the postgres container from workspace path
    PG_NAME=$(printf '%s' "$WORKSPACE" | cksum | awk '{print $1}')
}

# ── detect_tls ──────────────────────────────────────────────────────────
# Check for TLS certs. Checks .certs/ dir first, then falls back to
# COGNOS_TLS_CERT/KEY env vars (needed in worktrees where .certs/ is gitignored).
# Sets PROTO, exports COGNOS_TLS_CERT/KEY.
detect_tls() {
    local cert_dir="$PROJECT_DIR/.certs"
    if [ -f "$cert_dir/cert.pem" ] && [ -f "$cert_dir/key.pem" ]; then
        export COGNOS_TLS_CERT="$cert_dir/cert.pem"
        export COGNOS_TLS_KEY="$cert_dir/key.pem"
        PROTO="https"
    elif [ -f "$COGNOS_TLS_CERT" ] && [ -f "$COGNOS_TLS_KEY" ]; then
        PROTO="https"
    else
        PROTO="http"
    fi
}

# ── detect_vite_tls ────────────────────────────────────────────────────
# Detect Vite's protocol. Vite checks local .certs/ (vite.config.ts: ../../.certs),
# which may differ from PROTO (engine TLS via env vars — present in worktrees
# where .certs/ is gitignored). Sets VITE_PROTO. Must be called after FRONTEND_DIR.
detect_vite_tls() {
    local vite_cert_dir="$FRONTEND_DIR/../../.certs"
    if [ -f "$vite_cert_dir/cert.pem" ] && [ -f "$vite_cert_dir/key.pem" ]; then
        VITE_PROTO="https"
    elif [ -f "$COGNOS_TLS_CERT" ] && [ -f "$COGNOS_TLS_KEY" ]; then  # Vite's config also checks these env vars
        VITE_PROTO="https"
    else
        VITE_PROTO="http"
    fi
}

# ── setup_postgres ──────────────────────────────────────────────────────
# Start or verify Docker postgres container for workspace.
setup_postgres() {
    export COGNOS_WORKSPACE="$WORKSPACE"
    export COGNOS_PG_NAME="$PG_NAME"
    export COGNOS_PG_PORT="$PG_PORT"

    local need_restart=""
    if docker inspect "cognos-pg-$PG_NAME" >/dev/null 2>&1; then
        local container_status
        container_status=$(docker inspect --format='{{.State.Status}}' "cognos-pg-$PG_NAME" 2>/dev/null || echo "")
        if [ "$container_status" != "running" ]; then
            need_restart="container not running (status: $container_status)"
        else
            local actual_mount expected_mount
            actual_mount=$(docker inspect --format='{{range .Mounts}}{{if eq .Destination "/var/lib/postgresql/data"}}{{.Source}}{{end}}{{end}}' "cognos-pg-$PG_NAME" 2>/dev/null || echo "")
            expected_mount="$WORKSPACE/data/postgres"
            if [ "$actual_mount" != "$expected_mount" ]; then
                need_restart="mount mismatch (expected $expected_mount, got $actual_mount)"
            fi
        fi
    else
        need_restart="container does not exist"
    fi

    if [ -n "$need_restart" ]; then
        echo "PostgreSQL: $need_restart"
        _start_postgres_container
    else
        # Read the actual published port from the running container
        local actual_pg_port
        actual_pg_port=$(docker inspect --format='{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' "cognos-pg-$PG_NAME" 2>/dev/null || echo "")
        if [ -n "$actual_pg_port" ] && [ "$actual_pg_port" != "$PG_PORT" ]; then
            PG_PORT="$actual_pg_port"
            export PG_PORT
            export COGNOS_PG_PORT="$PG_PORT"
        fi
        echo "PostgreSQL already running for this workspace (port $PG_PORT)"
    fi
}

_start_postgres_container() {
    echo "Starting PostgreSQL for workspace: $WORKSPACE (port $PG_PORT, container cognos-pg-$PG_NAME)"

    docker rm -f "cognos-pg-$PG_NAME" 2>/dev/null || true

    docker compose -p "cognos-$PG_NAME" -f "$PROJECT_DIR/docker-compose.dev.yml" up -d

    echo -n "Waiting for PostgreSQL"
    for i in {1..30}; do
        if docker exec "cognos-pg-$PG_NAME" pg_isready -U cognos > /dev/null 2>&1; then
            echo " ready!"
            break
        fi
        echo -n "."
        sleep 1
    done

    docker exec "cognos-pg-$PG_NAME" psql -U cognos -d cognos -c "CREATE EXTENSION IF NOT EXISTS vector;" > /dev/null 2>&1 || true
}

# ── kill_stale_processes ────────────────────────────────────────────────
# Kill stale dev script processes and old frontend for this workspace.
# With -b: also kills the engine (need fresh build). Without -b: leaves
# a healthy engine running so multiple clients can share it.
kill_stale_processes() {
    local self_pid=$$
    local killed=""

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

    # With -b: kill existing engine so we start the freshly built one
    if [ -n "$BUILD" ]; then
        if [ -f "$ENGINE_PIDFILE" ]; then
            local old_pid
            old_pid="$(cat "$ENGINE_PIDFILE" 2>/dev/null || true)"
            if [ -n "$old_pid" ] && kill -0 "$old_pid" 2>/dev/null; then
                echo "Stopping existing engine for rebuild (PID $old_pid)..."
                kill "$old_pid" 2>/dev/null || true
                killed=1
            fi
            rm -f "$ENGINE_PIDFILE"
        fi

        # Kill any orphaned engine still holding our port
        if ! port_is_free "$API_PORT"; then
            local occupant occupant_cmd
            occupant=$(lsof -ti :"$API_PORT" 2>/dev/null || true)
            if [ -n "$occupant" ]; then
                occupant_cmd=$(ps -p "$occupant" -o comm= 2>/dev/null || true)
                if [[ "$occupant_cmd" == *cognos-engine* ]]; then
                    echo "Killing orphaned engine on port $API_PORT (PID $occupant)..."
                    kill "$occupant" 2>/dev/null || true
                    killed=1
                fi
            fi
        fi
    fi

    # Stop any existing frontend for THIS workspace (skip in --engine-only mode)
    if [ -z "$ENGINE_ONLY" ] && [ -f "$FRONTEND_PIDFILE" ]; then
        local old_pid
        old_pid="$(cat "$FRONTEND_PIDFILE" 2>/dev/null || true)"
        if [ -n "$old_pid" ] && kill -0 "$old_pid" 2>/dev/null; then
            echo "Stopping existing frontend for this workspace (PID $old_pid)..."
            kill "$old_pid" 2>/dev/null || true
            killed=1
        fi
        rm -f "$FRONTEND_PIDFILE"
    fi

    # Wait briefly for killed processes to release ports
    if [ -n "$killed" ]; then sleep 1; fi
}

# ── build_or_find_engine ────────────────────────────────────────────────
# Build engine if BUILD is set, otherwise find existing binary. Sets ENGINE_BIN.
build_or_find_engine() {
    if [ -n "$BUILD" ]; then
        # Kill IDE cargo check processes that hold the artifact directory lock
        local check_pids
        check_pids=$(pgrep -f 'cargo check' 2>/dev/null || true)
        if [ -n "$check_pids" ]; then
            echo "Killing cargo check processes to release build lock..."
            echo "$check_pids" | xargs kill 2>/dev/null || true
        fi

        # Remove stale lock files (can linger after sleep/wake with no holding process)
        rm -f "$PROJECT_DIR/target/.cargo-lock" "$PROJECT_DIR/target/debug/.cargo-lock" "$PROJECT_DIR/target/release/.cargo-lock" "$PROJECT_DIR/target/.package-cache"

        echo ""
        echo "Building engine..."
        # cognos-cli is built alongside the engine so the `cognos` binary
        # lands next to `cognos-engine`. The engine adds its directory to
        # PATH for spawned CC sessions; without the binary it skips that and
        # the cognos-cli skill is not installed.
        if [ -n "$RELEASE" ]; then
            cargo build -p cognos-engine -p cognos-cli --release
            ENGINE_BIN="$PROJECT_DIR/target/release/cognos-engine"
        else
            cargo build -p cognos-engine -p cognos-cli
            ENGINE_BIN="$PROJECT_DIR/target/debug/cognos-engine"
        fi
    else
        if [ -n "$RELEASE" ] && [ -f "$PROJECT_DIR/target/release/cognos-engine" ]; then
            ENGINE_BIN="$PROJECT_DIR/target/release/cognos-engine"
        elif [ -f "$PROJECT_DIR/target/debug/cognos-engine" ]; then
            ENGINE_BIN="$PROJECT_DIR/target/debug/cognos-engine"
        else
            echo "No engine binary found. Run with -b to build."
            exit 1
        fi
    fi
}

# ── swap_ports ──────────────────────────────────────────────────────────
# Engine takes VITE_PORT (user-facing), Vite runs on API_PORT (internal).
# Sets ENGINE_PORT, INTERNAL_VITE_PORT. Writes .cognos/ports. Exports env vars.
swap_ports() {
    INTERNAL_VITE_PORT="$API_PORT"
    ENGINE_PORT="$VITE_PORT"

    # Update ports file to reflect swapped assignments
    cat > "$WORKSPACE/.cognos/ports" <<EOF
API_PORT=$ENGINE_PORT
VITE_PORT=$ENGINE_PORT
EOF

    detect_vite_tls

    export COGNOS_API_PORT="$ENGINE_PORT"
    export DATABASE_URL="postgres://cognos:cognos@localhost:$PG_PORT/cognos"
    export WORKSPACE_PATH="$WORKSPACE"
    export COGNOS_DEV_PROXY="$VITE_PROTO://localhost:$INTERNAL_VITE_PORT"
}

source "$(dirname "${BASH_SOURCE[0]}")/sleep.sh"

# ── enable_clamshell_prevention ────────────────────────────────────────
enable_clamshell_prevention() {
    mkdir -p "$SLEEP_LOCK_DIR"
    cleanup_stale_sleep_locks

    local ws_hash
    ws_hash="$(echo -n "$WORKSPACE" | md5 -q)"
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
    caffeinate -im -w $$ &
    CAFFEINATE_PID=$!
    enable_clamshell_prevention
}

# ── start_engine ────────────────────────────────────────────────────────
# Run engine in background with caffeinate, write PID, wait for health (30s).
# Reuses an existing healthy engine if one is already running for this workspace.
# Sets ENGINE_PID.
start_engine() {
    # Check if an existing engine is already healthy on our port
    if [ -f "$ENGINE_PIDFILE" ]; then
        local existing_pid
        existing_pid="$(cat "$ENGINE_PIDFILE" 2>/dev/null || true)"
        if [ -n "$existing_pid" ] && kill -0 "$existing_pid" 2>/dev/null; then
            if curl -sk "$PROTO://localhost:$ENGINE_PORT/api/health" >/dev/null 2>&1; then
                echo ""
                echo "Reusing existing engine (PID $existing_pid) on port $ENGINE_PORT"
                ENGINE_PID="$existing_pid"
                start_caffeinate
                return
            fi
        fi
    fi

    echo ""
    echo "Starting CognOS engine..."
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
    "$ENGINE_BIN" >> "$ENGINE_LOG" 2>&1 &
    ENGINE_PID=$!
    echo $ENGINE_PID > "$ENGINE_PIDFILE"

    # Wait for engine to be ready
    echo -n "Waiting for engine"
    local engine_ready=""
    for i in {1..30}; do
        if ! kill -0 $ENGINE_PID 2>/dev/null; then
            echo ""
            echo "ERROR: Engine crashed on startup. Check logs:"
            echo "  tail -50 $ENGINE_LOG"
            tail -10 "$ENGINE_LOG"
            exit 1
        fi
        if curl -sk "$PROTO://localhost:$ENGINE_PORT/api/health" >/dev/null 2>&1; then
            echo " ready!"
            engine_ready="yes"
            break
        fi
        echo -n "."
        sleep 1
    done

    if [ -z "$engine_ready" ]; then
        echo ""
        echo "ERROR: Engine failed to start within 30 seconds. Check logs:"
        tail -20 "$ENGINE_LOG"
        exit 1
    fi
}

# ── running_frontend_workspaces ────────────────────────────────────────
# Echo workspace names whose frontend.pid points to a live process.
# Subshell so `nullglob` doesn't leak into the caller's shell options.
running_frontend_workspaces() (
    shopt -s nullglob
    for pidfile in "$HOME"/workspaces/*/.cognos/frontend.pid; do
        local pid ws_dir
        # `cat … || true` keeps a transient unreadable file from killing the
        # subshell under the caller's `set -e`.
        pid="$(cat "$pidfile" 2>/dev/null || true)"
        [ -n "$pid" ] || continue
        if kill -0 "$pid" 2>/dev/null; then
            ws_dir="${pidfile%/.cognos/frontend.pid}"
            echo "${ws_dir##*/}"
        fi
    done
)

# ── ensure_npm_deps ───────────────────────────────────────────────────
# Run npm install if node_modules is missing or package.json has changed.
# Refuses if any other workspace has a running frontend dev server, because
# mutating node_modules under a running Vite silently corrupts its in-memory
# module graph (stale inodes) and turns it into a wedged "200 with wrong body"
# server — manifesting as a blank page in the browser.
# Usage: ensure_npm_deps "path/to/package" "label"
ensure_npm_deps() {
    local dir="$1"
    local label="${2:-dependencies}"
    local needs_install=""

    if [ ! -d "$dir/node_modules" ]; then
        needs_install="node_modules missing"
    elif [ "$dir/package.json" -nt "$dir/node_modules" ]; then
        # node_modules dir mtime is updated by every `npm install` (it always
        # rewrites .package-lock.json). The local .package-lock.json file isn't
        # reliable in npm-workspace setups — installs at the root don't touch
        # the per-package lockfile, so it's permanently "stale" and would
        # trigger spurious reinstalls every startup.
        needs_install="package.json changed"
    fi

    if [ -n "$needs_install" ]; then
        local active
        active="$(running_frontend_workspaces)"
        if [ -n "$active" ]; then
            echo "" >&2
            echo "ERROR: $label install needed ($needs_install), but a frontend is running for:" >&2
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
        (cd "$dir" && npm install)
    fi
}

# ── ensure_frontend_deps ───────────────────────────────────────────────
# Run npm install if node_modules is missing or package.json has changed
# since the last install. Safe to call from both web-dev and tauri-dev.
ensure_frontend_deps() {
    ensure_npm_deps "$FRONTEND_DIR" "frontend dependencies"
}

# ── build_sdk ──────────────────────────────────────────────────────────
# Build the CognOS SDK bundle (packages/cognos-sdk → dist/sdk.js).
# The engine serves this at /api/v1/sdk.js for app UIs.
build_sdk() {
    # SDK deps are hoisted to root node_modules by npm workspaces
    ensure_npm_deps "$PROJECT_DIR" "workspace dependencies"
    echo "Building CognOS SDK..."
    (cd "$PROJECT_DIR/packages/cognos-sdk" && npm run build)
}

# ── start_vite ──────────────────────────────────────────────────────────
# Install npm deps if needed, start Vite on internal port, wait for ready (60s).
# Sets FRONTEND_PID.
start_vite() {
    ensure_frontend_deps

    echo "Starting Vite dev server..."
    export VITE_PORT="$INTERNAL_VITE_PORT"
    export API_PORT="$ENGINE_PORT"
    (cd "$FRONTEND_DIR" && npx vite --host --port "$INTERNAL_VITE_PORT") > /dev/null 2>&1 &
    FRONTEND_PID=$!
    echo $FRONTEND_PID > "$FRONTEND_PIDFILE"

    detect_vite_tls

    # Probe `/@vite/client` instead of `/`. A wedged Vite (e.g. node_modules
    # mutated under it) still returns 200 on `/` with the SPA index, so the
    # old probe passed for a server that would serve a blank page. The Vite
    # internal `/@vite/client` endpoint must return text/javascript — anything
    # else means Vite is broken even if it's listening.
    local probe_url="$VITE_PROTO://localhost:$INTERNAL_VITE_PORT/@vite/client"
    echo -n "Waiting for Vite"
    local vite_ready="" content_type=""
    # Wall-clock budget: per-curl `--max-time 2` means an iteration count would
    # be misleading. SECONDS reflects real elapsed seconds.
    local deadline=$((SECONDS + 60))
    while (( SECONDS < deadline )); do
        # `|| true` keeps curl's exit 7 (connect refused while Vite spins up) from
        # tripping `set -e` through the assignment on macOS bash 3.2.
        content_type=$(curl -sk --connect-timeout 1 --max-time 2 -o /dev/null -w "%{content_type}" "$probe_url" 2>/dev/null || true)
        if [[ "$content_type" == text/javascript* || "$content_type" == application/javascript* ]]; then
            echo " ready!"
            vite_ready="yes"
            break
        fi
        echo -n "."
        sleep 1
    done

    if [ -z "$vite_ready" ]; then
        echo ""
        echo "ERROR: Vite did not become ready within 60 seconds." >&2
        echo "  $probe_url returned content-type: ${content_type:-<no response>}" >&2
        echo "  (expected text/javascript — Vite is wedged, killing it)" >&2
        kill "$FRONTEND_PID" 2>/dev/null || true
        rm -f "$FRONTEND_PIDFILE"
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

    # Detect OCR
    local ocr_status="Not available (install with: brew install tesseract poppler)"
    if command -v pdftoppm >/dev/null 2>&1 && command -v tesseract >/dev/null 2>&1; then
        ocr_status="Available (tesseract + poppler)"
    fi

    # Detect network
    local local_ip ts_hostname
    local_ip=$(ipconfig getifaddr en0 2>/dev/null || echo "unknown")
    ts_hostname=$(tailscale status --self --json 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['Self'].get('DNSName','').rstrip('.'))" 2>/dev/null || echo "")

    echo ""
    echo "========================================"
    if [ "$mode" = "tauri" ]; then
        echo "  CognOS Tauri dev ready"
    elif [ "$mode" = "engine-only" ]; then
        echo "  CognOS engine restarted"
    else
        echo "  CognOS dev server ready"
    fi
    echo "  Workspace:   $WORKSPACE"
    echo "  Local:       $PROTO://localhost:$ENGINE_PORT"
    echo "  Network:     $PROTO://$local_ip:$ENGINE_PORT"
    if [ -n "$ts_hostname" ]; then
        echo "  Tailscale:   $PROTO://$ts_hostname:$ENGINE_PORT"
    fi
    echo "  Vite HMR:    $PROTO://localhost:$INTERNAL_VITE_PORT"
    echo "  PostgreSQL:  localhost:$PG_PORT"
    echo "  Engine:      Native macOS"
    echo "  OCR:         $ocr_status"
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
    if [ -f "$FRONTEND_PIDFILE" ]; then
        kill "$(cat "$FRONTEND_PIDFILE" 2>/dev/null)" 2>/dev/null || true
        rm -f "$FRONTEND_PIDFILE"
    fi
    echo "Engine still running for workspace: $WORKSPACE (port $ENGINE_PORT)"
    echo "Stop with: ./scripts/stop.sh -w $WORKSPACE"
    echo "PostgreSQL still running (container cognos-pg-$PG_NAME). Stop with:"
    echo "  ./scripts/stop.sh -w $WORKSPACE --force"
}
