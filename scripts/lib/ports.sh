#!/bin/bash
# Port allocation functions for multi-workspace support.
# Each workspace gets a stable, incrementing port offset (0, 1, 2, ...) stored
# in a global registry at ~/.lucidos/port-registry.
# Ports: API=3000+offset, Vite=5173+offset, PostgreSQL=5432+offset

LUCIDOS_PORT_REGISTRY="$HOME/.lucidos/port-registry"

# Check if a port is available (not in use by another process).
port_is_free() {
    local port="$1"
    ! lsof -ti :"$port" >/dev/null 2>&1
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
    local max
    max=$(awk -F'\t' 'BEGIN{m=-1} {if($2+0>m) m=$2+0} END{print m}' "$LUCIDOS_PORT_REGISTRY")
    echo $(( max + 1 ))
}

# Register a workspace with an offset in the global registry.
registry_save() {
    local workspace="$1"
    local offset="$2"
    mkdir -p "$(dirname "$LUCIDOS_PORT_REGISTRY")"
    # Remove any existing entry for this workspace, then append
    if [ -f "$LUCIDOS_PORT_REGISTRY" ]; then
        local tmp="$LUCIDOS_PORT_REGISTRY.tmp"
        awk -F'\t' -v ws="$workspace" '$1 != ws' "$LUCIDOS_PORT_REGISTRY" > "$tmp"
        mv "$tmp" "$LUCIDOS_PORT_REGISTRY"
    fi
    printf '%s\t%s\n' "$workspace" "$offset" >> "$LUCIDOS_PORT_REGISTRY"
}

# Allocate ports for a workspace.
# Looks up stable offset from global registry (~/.lucidos/port-registry).
# New workspaces get the next incrementing offset (0, 1, 2, ...).
# Exports: API_PORT, VITE_PORT, PG_PORT
allocate_ports() {
    local workspace="$1"
    local ports_file="$workspace/.lucidos/ports"

    mkdir -p "$workspace/.lucidos"

    # Look up or assign offset from global registry
    local offset
    offset=$(registry_lookup "$workspace")
    if [ -z "$offset" ]; then
        offset=$(registry_next_offset)
        registry_save "$workspace" "$offset"
    fi

    API_PORT=$(( 3000 + offset ))
    VITE_PORT=$(( 5173 + offset ))
    PG_PORT=$(( 5432 + offset ))

    # Ensure ports are free — kill stale processes if needed (skip our own PG container)
    local pg_name
    pg_name=$(printf '%s' "$workspace" | cksum | awk '{print $1}')
    local engine_pid_file="$workspace/.lucidos/engine.pid"
    local frontend_pid_file="$workspace/.lucidos/frontend.pid"

    # In --engine-only mode, only clean up the engine port (VITE_PORT, which becomes
    # ENGINE_PORT after swap). API_PORT has Vite running — leave it alone.
    local ports_to_check="$API_PORT $VITE_PORT $PG_PORT"
    if [ -n "$ENGINE_ONLY" ]; then
        ports_to_check="$VITE_PORT"
    fi

    for port in $ports_to_check; do
        port_is_free "$port" && continue

        # Check if it's our PG container — leave it alone
        if [ "$port" = "$PG_PORT" ]; then
            local container_port
            container_port=$(docker inspect --format='{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' "lucidos-pg-$pg_name" 2>/dev/null || echo "")
            [ "$container_port" = "$port" ] && continue
        fi

        # Check if it's our own engine or frontend — leave it alone
        local port_pid
        port_pid=$(lsof -ti :"$port" 2>/dev/null | head -1)
        if [ -n "$port_pid" ]; then
            if [ -f "$engine_pid_file" ] && [ "$port_pid" = "$(cat "$engine_pid_file" 2>/dev/null)" ]; then
                continue
            elif [ -f "$frontend_pid_file" ] && [ "$port_pid" = "$(cat "$frontend_pid_file" 2>/dev/null)" ]; then
                continue
            fi
        fi

        # Stale process — kill it gracefully first, then force if needed
        echo "Killing stale process on port $port (pid $port_pid)..." >&2
        local stale_pids
        stale_pids=$(lsof -ti :"$port" 2>/dev/null)
        echo "$stale_pids" | xargs kill 2>/dev/null || true

        # Wait up to 3 seconds for graceful shutdown
        for i in 1 2 3; do
            port_is_free "$port" && break
            sleep 1
        done

        # Force kill if still alive
        if ! port_is_free "$port"; then
            stale_pids=$(lsof -ti :"$port" 2>/dev/null || true)
            echo "$stale_pids" | xargs kill -9 2>/dev/null || true
        fi
    done

    # Verify non-PG ports are free after cleanup (only check ports we tried to clean)
    local verify_ports="$API_PORT $VITE_PORT"
    if [ -n "$ENGINE_ONLY" ]; then
        verify_ports="$VITE_PORT"
    fi
    for port in $verify_ports; do
        if ! port_is_free "$port"; then
            sleep 1
            if ! port_is_free "$port"; then
                # Last resort: force kill
                lsof -ti :"$port" 2>/dev/null | xargs kill -9 2>/dev/null || true
                sleep 1
                if ! port_is_free "$port"; then
                    echo "ERROR: Port $port still occupied after cleanup." >&2
                    return 1
                fi
            fi
        fi
    done

    # Save to workspace-local ports file
    cat > "$ports_file" <<EOF
API_PORT=$API_PORT
VITE_PORT=$VITE_PORT
EOF
    export API_PORT VITE_PORT PG_PORT
    return 0
}
