#!/bin/bash
# Check CognOS engine status (supports multiple concurrent workspaces)
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

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
            echo "Without -w: scans for all running CognOS workspaces."
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# Show status for a single workspace
show_workspace_status() {
    local ws="$1"
    local engine_pid_file="$ws/.cognos/engine.pid"
    local frontend_pid_file="$ws/.cognos/frontend.pid"
    local ports_file="$ws/.cognos/ports"
    local engine_log="$ws/.cognos/engine.log"

    echo "  Workspace: $ws"

    # Load ports (API + Vite from file, PG from Docker container)
    local api_port="" vite_port="" pg_port=""
    if [ -f "$ports_file" ]; then
        source "$ports_file"
        api_port="$API_PORT"
        vite_port="$VITE_PORT"
    fi

    # Read PG port from container
    local pg_name
    pg_name=$(printf '%s' "$ws" | cksum | awk '{print $1}')
    pg_port=$(docker inspect --format='{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' "cognos-pg-$pg_name" 2>/dev/null || echo "")

    if [ -n "$api_port" ]; then
        echo "  Ports:     API=$api_port  Vite=$vite_port  PG=${pg_port:-?}"
    fi

    # Engine status
    if [ -f "$engine_pid_file" ]; then
        local pid
        pid="$(cat "$engine_pid_file")"
        if kill -0 "$pid" 2>/dev/null; then
            echo "  Engine:    RUNNING (PID $pid)"
        else
            echo "  Engine:    STOPPED (stale pidfile, PID $pid)"
        fi
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
        done < <(pgrep -x cognos-app 2>/dev/null || true)
    fi
    if [ -z "$tauri_found" ]; then
        echo "  Tauri:     STOPPED"
    fi

    # PostgreSQL status
    if docker inspect "cognos-pg-$pg_name" >/dev/null 2>&1; then
        local container_status
        container_status=$(docker inspect --format='{{.State.Status}}' "cognos-pg-$pg_name" 2>/dev/null || echo "unknown")
        echo "  PostgreSQL: $container_status (container cognos-pg-$pg_name, port ${pg_port:-?})"
    else
        echo "  PostgreSQL: no container"
    fi

    # API health
    if [ -n "$api_port" ]; then
        if curl -sk "https://localhost:$api_port/api/health" >/dev/null 2>&1 || curl -s "http://localhost:$api_port/api/health" >/dev/null 2>&1; then
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
    local ports_file="$ws/.cognos/ports"
    local engine_pid_file="$ws/.cognos/engine.pid"

    # Load ports
    local api_port="" vite_port="" pg_port=""
    if [ -f "$ports_file" ]; then
        source "$ports_file"
        api_port="$API_PORT"
        vite_port="$VITE_PORT"
    fi

    # PG port from container
    local pg_name
    pg_name=$(printf '%s' "$ws" | cksum | awk '{print $1}')
    pg_port=$(docker inspect --format='{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' "cognos-pg-$pg_name" 2>/dev/null || echo "")

    # Engine running?
    local engine_running="false"
    if [ -f "$engine_pid_file" ]; then
        local pid
        pid="$(cat "$engine_pid_file")"
        if kill -0 "$pid" 2>/dev/null; then
            engine_running="true"
        fi
    fi

    # Workspace name
    local name
    name=$(basename "$ws")

    # Engine version from health endpoint
    local engine_version=""
    if [ "$engine_running" = "true" ] && [ -n "$api_port" ]; then
        engine_version=$(curl -sk --connect-timeout 2 --max-time 3 "https://localhost:$api_port/api/health" 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin).get('engine_version',''))" 2>/dev/null || echo "")
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
        if [ -d "$WORKSPACE" ]; then
            EXCLUDE_WS="$(cd "$WORKSPACE" && pwd)"
        else
            EXCLUDE_WS="$WORKSPACE"
        fi
    fi

    while IFS= read -r container; do
        [ -z "$container" ] && continue
        ws_dir=$(docker inspect --format='{{range .Mounts}}{{if eq .Destination "/var/lib/postgresql/data"}}{{.Source}}{{end}}{{end}}' "$container" 2>/dev/null || echo "")
        if [ -n "$ws_dir" ]; then
            ws_dir="$(dirname "$(dirname "$ws_dir")")"
            if [ -d "$ws_dir/.cognos" ]; then
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
    done < <(docker ps --filter "name=cognos-pg-" --format '{{.Names}}' 2>/dev/null || true)

    printf '{"workspaces":[%s]}\n' "$ENTRIES"
    exit 0
fi

echo "=== CognOS Engine Status ==="
echo ""

if [ -n "$WORKSPACE" ]; then
    # Resolve to absolute path
    if [ -d "$WORKSPACE" ]; then
        WORKSPACE="$(cd "$WORKSPACE" && pwd)"
    fi
    show_workspace_status "$WORKSPACE"
else
    # Find workspaces by inspecting running cognos PG containers
    FOUND=""
    while IFS= read -r container; do
        [ -z "$container" ] && continue
        # Extract workspace path from the postgres data mount
        ws_dir=$(docker inspect --format='{{range .Mounts}}{{if eq .Destination "/var/lib/postgresql/data"}}{{.Source}}{{end}}{{end}}' "$container" 2>/dev/null || echo "")
        # Mount points to <workspace>/data/postgres — go up two levels
        if [ -n "$ws_dir" ]; then
            ws_dir="$(dirname "$(dirname "$ws_dir")")"
            if [ -d "$ws_dir/.cognos" ]; then
                if [ -n "$FOUND" ]; then
                    echo "---"
                fi
                FOUND="1"
                show_workspace_status "$ws_dir"
                echo ""
            fi
        fi
    done < <(docker ps --filter "name=cognos-pg-" --format '{{.Names}}' 2>/dev/null || true)

    if [ -z "$FOUND" ]; then
        echo "No CognOS workspaces found."
        echo ""
        echo "Start one with: ./scripts/web-dev.sh -w <workspace>"
    fi
fi

# Environment
echo ""
echo "Environment:"
echo "  VERTEX_PROJECT_ID: ${VERTEX_PROJECT_ID:-<not set>}"
echo "  VERTEX_REGION:     ${VERTEX_REGION:-europe-west1}"
