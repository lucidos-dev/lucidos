#!/bin/bash
# Check Lucidos engine status (supports multiple concurrent workspaces)
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

source "$SCRIPT_DIR/lib/workspace.sh"

_workspace_db_for_path() {
    local ws="$1" saved="${WORKSPACE:-}" db
    WORKSPACE="$ws"
    db="$(workspace_database_name)"
    WORKSPACE="$saved"
    echo "$db"
}

_shared_pg_port() {
    docker inspect --format='{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' "$(shared_pg_container)" 2>/dev/null || echo ""
}

_iter_workspace_dirs() {
    local reg
    reg="$(gateway_data_dir)/config/workspaces.json"
    if [ -f "$reg" ]; then
        python3 - "$reg" "$(gateway_data_dir)" <<'PY'
import json, os, sys
reg, base = sys.argv[1:3]
try:
    data = json.load(open(reg))
except Exception:
    data = {}
for w in data.get("workspaces", []):
    d = w.get("dir")
    if not d:
        continue
    if not os.path.isabs(d):
        d = os.path.join(base, d)
    print(d)
PY
        return
    fi
    for d in "$HOME"/workspaces/*; do
        [ -d "$d/.lucidos" ] && echo "$d"
    done
}

# Probe the engine /health endpoint for a workspace's API port. Echoes the
# raw JSON body and returns 0 when the engine answers; returns non-zero when
# unreachable (or no port given). This is the reachability source of truth
# that reconciles a stale .lucidos/engine.pid against an engine that is in
# fact serving — the pidfile only tracks whether THIS host's supervised
# process is alive, which goes stale across a restart (old PID dead in the
# file for a beat, or an engine (re)started by a path that didn't refresh it).
# See the callers in show_workspace_status / json_workspace_status.
_engine_health_json() {
    local port="$1" out
    [ -z "$port" ] && return 1
    if out=$(curl -sk --connect-timeout 2 --max-time 3 "https://localhost:$port/api/v1/health" 2>/dev/null); then
        printf '%s' "$out"
        return 0
    fi
    if out=$(curl -s --connect-timeout 2 --max-time 3 "http://localhost:$port/api/v1/health" 2>/dev/null); then
        printf '%s' "$out"
        return 0
    fi
    return 1
}

# Parse arguments
WORKSPACE=""
JSON_MODE=""
while [[ $# -gt 0 ]]; do
    case $1 in
        -w|--workspace) WORKSPACE="$2"; shift 2 ;;
        --json) JSON_MODE="1"; shift ;;
        -h|--help)
            echo "Usage: $0 [-w <workspace>] [--json]"
            echo ""
            echo "Options:"
            echo "  -w, --workspace DIR   Show status for a specific workspace"
            echo "  --json                Output structured JSON (for API consumption)"
            echo "  -h, --help            Show this help"
            echo ""
            echo "Without -w: scans for all running Lucidos workspaces."
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# Show status for a single workspace
show_workspace_status() {
    local ws="$1"
    local engine_pid_file="$ws/.lucidos/engine.pid"
    local frontend_pid_file="$ws/.lucidos/frontend.pid"
    local ports_file="$ws/.lucidos/ports"
    local engine_log="$ws/.lucidos/engine.log"

    echo "  Workspace: $ws"

    # Load ports (API + Vite + shared PG from file when available).
    local api_port="" vite_port="" pg_port="" pg_database=""
    if [ -f "$ports_file" ]; then
        # shellcheck disable=SC1090 # <ws>/.lucidos/ports is written at runtime by allocate_ports (lib/ports.sh)
        source "$ports_file"
        api_port="${API_PORT:-}"
        vite_port="${VITE_PORT:-}"
        pg_port="${PG_PORT:-}"
        pg_database="${PG_DATABASE:-}"
    fi
    [ -n "$pg_port" ] || pg_port="$(_shared_pg_port)"
    [ -n "$pg_database" ] || pg_database="$(_workspace_db_for_path "$ws")"

    if [ -n "$api_port" ]; then
        echo "  Ports:     API=$api_port  Vite=$vite_port  PG=${pg_port:-?}"
    fi

    # Probe /health once, up front — both the Engine line and the API line
    # below read this. A responding /health is the truth for "is it serving";
    # the pidfile only tracks this host's supervised process, which goes stale
    # across a restart. Reconciling the two stops a serving engine from
    # reading "STOPPED" directly above an "API: ... (healthy)" line.
    local api_healthy=""
    if [ -n "$api_port" ] && _engine_health_json "$api_port" >/dev/null; then
        api_healthy="1"
    fi

    # Engine status: reachability first, then the pidfile. Serving is what
    # "running" means to a caller; the pid only adds detail. A pid that is alive
    # but NOT serving gets its own line rather than a bare RUNNING, because that
    # is a boot in progress or a wedged engine, and either way the workspace
    # can't be opened yet. `pid_is_live` (lib/workspace.sh) is zombie-aware; a
    # plain `kill -0` reports a defunct engine as alive.
    if [ -f "$engine_pid_file" ]; then
        local pid
        pid="$(cat "$engine_pid_file")"
        if [ -n "$api_healthy" ]; then
            if pid_is_live "$pid"; then
                echo "  Engine:    RUNNING (PID $pid)"
            else
                echo "  Engine:    RUNNING (serving on $api_port; pidfile PID $pid is stale)"
            fi
        elif pid_is_live "$pid"; then
            echo "  Engine:    NOT SERVING (PID $pid alive, no /health on ${api_port:-?})"
        else
            echo "  Engine:    STOPPED (stale pidfile, PID $pid)"
        fi
    elif [ -n "$api_healthy" ]; then
        echo "  Engine:    RUNNING (serving on $api_port; no pidfile)"
    else
        echo "  Engine:    STOPPED"
    fi

    # Frontend status
    if [ -f "$frontend_pid_file" ]; then
        local pid
        pid="$(cat "$frontend_pid_file")"
        if pid_is_live "$pid"; then
            echo "  Frontend:  RUNNING (PID $pid, http://localhost:${vite_port:-?})"
            # Show LAN IP
            local lan_ip
            lan_ip=$(ipconfig getifaddr en0 2>/dev/null || echo "")
            if [ -n "$lan_ip" ] && [ -n "$vite_port" ]; then
                echo "  Network:   https://${lan_ip}:${vite_port}"
            fi
            # Show Tailscale HTTPS URL if available
            local ts_dns
            ts_dns=$(tailscale status --self --json 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin).get('Self',{}).get('DNSName','').rstrip('.'))" 2>/dev/null)
            if [ -n "$ts_dns" ] && [ -n "$vite_port" ]; then
                echo "  Tailscale: https://${ts_dns}:${vite_port}"
            fi
        else
            echo "  Frontend:  STOPPED (stale pidfile)"
        fi
    else
        echo "  Frontend:  STOPPED"
    fi

    # Tauri app status — match by engine port in parent cargo-tauri process
    local tauri_found=""
    if [ -n "$api_port" ]; then
        while IFS= read -r tpid; do
            [ -z "$tpid" ] && continue
            local ppid_cmd
            ppid_cmd=$(ps -p "$(ps -p "$tpid" -o ppid= 2>/dev/null | tr -d ' ')" -o command= 2>/dev/null || true)
            if [[ "$ppid_cmd" == *"localhost:${api_port}"* ]]; then
                echo "  Tauri:     RUNNING (PID $tpid)"
                tauri_found="1"
                break
            fi
        done < <(pgrep -x lucidos-app 2>/dev/null || true)
    fi
    if [ -z "$tauri_found" ]; then
        echo "  Tauri:     STOPPED"
    fi

    # PostgreSQL status. Steady state is one shared container + one database per
    # workspace. A legacy per-workspace container may still exist as rollback
    # until explicitly decommissioned.
    if docker inspect "$(shared_pg_container)" >/dev/null 2>&1; then
        local container_status
        container_status=$(docker inspect --format='{{.State.Status}}' "$(shared_pg_container)" 2>/dev/null || echo "unknown")
        echo "  PostgreSQL: $container_status shared (container $(shared_pg_container), port ${pg_port:-?}, database ${pg_database:-?})"
    else
        echo "  PostgreSQL: shared container not found"
    fi
    local pg_name
    pg_name=$(printf '%s' "$ws" | cksum | awk '{print $1}')
    if docker inspect "lucidos-pg-$pg_name" >/dev/null 2>&1; then
        local legacy_status
        legacy_status=$(docker inspect --format='{{.State.Status}}' "lucidos-pg-$pg_name" 2>/dev/null || echo "unknown")
        echo "  Legacy PG:  $legacy_status (container lucidos-pg-$pg_name; kept for rollback)"
    fi

    # API health — reuse the single probe from the top of this function.
    if [ -n "$api_port" ]; then
        if [ -n "$api_healthy" ]; then
            echo "  API:       localhost:$api_port (healthy)"
        else
            echo "  API:       not responding"
        fi
    fi

    # Workspace stats
    if [ -d "$ws" ]; then
        if [ -d "$ws/data/artifacts" ]; then
            local artifact_count
            artifact_count=$(find "$ws/data/artifacts" -type f 2>/dev/null | wc -l | tr -d ' ')
            echo "  Artifacts: $artifact_count files"
        fi
        if [ -d "$ws/data/skills" ]; then
            local skill_count
            skill_count=$(find "$ws/data/skills" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
            echo "  Skills:    $skill_count"
        fi
    fi

    # Log file
    if [ -f "$engine_log" ]; then
        echo "  Log:       $engine_log"
    fi
}

# JSON output for a single workspace — prints a JSON object (no trailing comma)
json_workspace_status() {
    local ws="$1"
    local ports_file="$ws/.lucidos/ports"

    # Load ports
    local api_port="" vite_port="" pg_port=""
    if [ -f "$ports_file" ]; then
        # shellcheck disable=SC1090 # <ws>/.lucidos/ports is written at runtime by allocate_ports (lib/ports.sh)
        source "$ports_file"
        api_port="${API_PORT:-}"
        vite_port="${VITE_PORT:-}"
    fi

    # Workspace name
    local name
    name=$(basename "$ws")

    # `engine_running` is REACHABILITY, and nothing else: one health probe,
    # which also yields the engine version in the same call. Both consumers
    # (cross-workspace open, the control-panel switcher dot) navigate straight
    # to the port, so "can I open this right now" is the only useful answer.
    #
    # The pidfile deliberately does NOT feed this field. It cannot answer that
    # question and it lies in both directions: a live pid may not be serving
    # yet, `kill -0` succeeds for a ZOMBIE, and a recycled pid belongs to an
    # unrelated process. On 2026-07-31 a defunct engine pid held this field at
    # `true` for a day (with `engine_version` empty in the same row, because
    # /health never answered), which the switcher rendered as a healthy dot
    # pointing at a dead port.
    local engine_running="false" engine_version="" health_body=""
    if [ -n "$api_port" ] && health_body=$(_engine_health_json "$api_port"); then
        engine_running="true"
        engine_version=$(printf '%s' "$health_body" | python3 -c "import sys,json; print(json.load(sys.stdin).get('engine_version',''))" 2>/dev/null || echo "")
    fi

    # Build JSON — use jq for safe escaping of paths/names
    jq -n \
        --arg name "$name" \
        --arg path "$ws" \
        --argjson port "${api_port:-null}" \
        --argjson engine_running "$engine_running" \
        --arg engine_version "$engine_version" \
        -c '{name: $name, path: $path, port: $port, engine_running: $engine_running, engine_version: $engine_version}'
}

# --- JSON mode ---
# Lists every workspace, including the caller's own — the control panel renders
# the current workspace as the active row with a refresh control (parity with
# the gateway picker), so the engine no longer passes -w to exclude itself.
if [ -n "$JSON_MODE" ]; then
    ENTRIES=""

    while IFS= read -r ws_dir; do
        [ -z "$ws_dir" ] && continue
        if [ -n "$ws_dir" ]; then
            if [ -d "$ws_dir/.lucidos" ]; then
                if [ -n "$ENTRIES" ]; then
                    ENTRIES="$ENTRIES,"
                fi
                ENTRIES="$ENTRIES$(json_workspace_status "$ws_dir")"
            fi
        fi
    done < <(_iter_workspace_dirs)

    printf '{"workspaces":[%s]}\n' "$ENTRIES"
    exit 0
fi

echo "=== Lucidos Engine Status ==="
echo ""

if [ -n "$WORKSPACE" ]; then
    resolve_workspace_path
    show_workspace_status "$WORKSPACE"
else
    # Find workspaces from the shared gateway registry / workspace dirs. A
    # shared Postgres container no longer identifies individual workspaces.
    FOUND=""
    while IFS= read -r ws_dir; do
        [ -z "$ws_dir" ] && continue
        if [ -n "$ws_dir" ]; then
            if [ -d "$ws_dir/.lucidos" ]; then
                if [ -n "$FOUND" ]; then
                    echo "---"
                fi
                FOUND="1"
                show_workspace_status "$ws_dir"
                echo ""
            fi
        fi
    done < <(_iter_workspace_dirs)

    if [ -z "$FOUND" ]; then
        echo "No Lucidos workspaces found."
        echo ""
        echo "Start one with: ./scripts/web-dev.sh -w <workspace>"
    fi
fi

# Environment
echo ""
echo "Environment:"
echo "  VERTEX_PROJECT_ID: ${VERTEX_PROJECT_ID:-<not set>}"
echo "  VERTEX_REGION:     ${VERTEX_REGION:-europe-west1}"
