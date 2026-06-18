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

# Resolve a container's workspace directory.
# Prefers the `lucidos.workspace` label (set by docker-compose.dev.yml).
# Falls back to the bind-mount source for legacy pre-named-volume containers
# (where the host path was <workspace>/data/postgres → strip last two segments).
_inspect_workspace_dir() {
    local container="$1"
    local ws_dir
    ws_dir=$(docker inspect --format='{{index .Config.Labels "lucidos.workspace"}}' "$container" 2>/dev/null || echo "")
    if [ -n "$ws_dir" ]; then
        echo "$ws_dir"
        return
    fi
    local mount_src
    mount_src=$(docker inspect --format='{{range .Mounts}}{{if eq .Destination "/var/lib/postgresql/data"}}{{if eq .Type "bind"}}{{.Source}}{{end}}{{end}}{{end}}' "$container" 2>/dev/null || echo "")
    if [ -n "$mount_src" ]; then
        echo "$(dirname "$(dirname "$mount_src")")"
    fi
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
        source "$ports_file"
        api_port="$API_PORT"
        vite_port="$VITE_PORT"
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

    # Engine status — pidfile liveness reconciled against reachability.
    if [ -f "$engine_pid_file" ]; then
        local pid
        pid="$(cat "$engine_pid_file")"
        if kill -0 "$pid" 2>/dev/null; then
            echo "  Engine:    RUNNING (PID $pid)"
        elif [ -n "$api_healthy" ]; then
            echo "  Engine:    RUNNING (serving on $api_port; pidfile PID $pid is stale)"
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
        if kill -0 "$pid" 2>/dev/null; then
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
    local engine_pid_file="$ws/.lucidos/engine.pid"

    # Load ports
    local api_port="" vite_port="" pg_port=""
    if [ -f "$ports_file" ]; then
        source "$ports_file"
        api_port="$API_PORT"
        vite_port="$VITE_PORT"
    fi

    # Engine running? Pidfile liveness OR a responding /health. Consumers of
    # this field (cross-workspace open, the control-panel status dot) hit the
    # port directly, so reachability is the truth they need — a stale pidfile
    # on a serving engine must not read as "not running".
    local engine_running="false"
    if [ -f "$engine_pid_file" ]; then
        local pid
        pid="$(cat "$engine_pid_file")"
        if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
            engine_running="true"
        fi
    fi

    # Workspace name
    local name
    name=$(basename "$ws")

    # One health probe: confirms reachability (overrides a stale pidfile) and
    # yields the engine version in the same call.
    local engine_version="" health_body=""
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
if [ -n "$JSON_MODE" ]; then
    ENTRIES=""
    EXCLUDE_WS=""
    if [ -n "$WORKSPACE" ]; then
        # The exclude is opportunistic — engines pass an absolute workspace_path
        # so it normally resolves; a missing shortname shouldn't be fatal in
        # JSON mode (returns no exclude → may include the caller, harmless).
        resolve_workspace_path 2>/dev/null || true
        EXCLUDE_WS="$WORKSPACE"
    fi

    while IFS= read -r ws_dir; do
        [ -z "$ws_dir" ] && continue
        if [ -n "$ws_dir" ]; then
            if [ -d "$ws_dir/.lucidos" ]; then
                # Skip the requesting workspace
                if [ -n "$EXCLUDE_WS" ] && [ "$ws_dir" = "$EXCLUDE_WS" ]; then
                    continue
                fi
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
