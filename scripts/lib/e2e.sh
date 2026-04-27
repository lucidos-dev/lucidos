#!/bin/bash

E2E_WORKSPACE="$HOME/workspaces/e2e-test"

# Resolve paths: scripts/lib/e2e.sh → scripts/lib/ → scripts/ → project root
_E2E_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
_E2E_SCRIPTS_DIR="$(dirname "$_E2E_LIB_DIR")"
_E2E_PROJECT_DIR="$(dirname "$_E2E_SCRIPTS_DIR")"

# Use mock LLM provider by default for e2e tests (override with LUCIDOS_MODEL=... before calling)
export LUCIDOS_MODEL="${LUCIDOS_MODEL:-mock}"

# Source shared infrastructure — provides detect_tls, setup_postgres, start_engine,
# start_vite, etc. Set the globals workspace.sh expects from its caller.
SCRIPT_DIR="$_E2E_SCRIPTS_DIR"
PROJECT_DIR="$_E2E_PROJECT_DIR"
FRONTEND_DIR="$_E2E_PROJECT_DIR/crates/lucidos-app"
SCRIPT_NAME="e2e"

source "$_E2E_LIB_DIR/ports.sh"
source "$_E2E_LIB_DIR/workspace.sh"
source "$_E2E_LIB_DIR/e2e_lock.sh"

# ── ensure_workspace_running ────────────────────────────────────────────
# Starts the e2e workspace if not running. Ensures both engine AND Vite are up.
# Uses LUCIDOS_MODEL=mock by default so tests don't hit real LLM APIs.
ensure_workspace_running() {
    # Set up workspace globals (pidfiles, log path, PG_NAME)
    export WORKSPACE="$E2E_WORKSPACE"
    resolve_workspace

    # Allocate ports and detect TLS
    allocate_ports "$WORKSPACE"
    detect_tls

    # After allocate_ports: API_PORT = internal Vite port, VITE_PORT = engine port
    local engine_port="$VITE_PORT"
    local vite_port="$API_PORT"

    # ── Engine ──
    if curl -sk "${PROTO}://localhost:${engine_port}/api/health" >/dev/null 2>&1; then
        echo "Engine already running on port $engine_port"
        # Set up env vars that swap_ports normally provides
        swap_ports
    else
        echo "Starting e2e workspace (LUCIDOS_MODEL=$LUCIDOS_MODEL)..."
        setup_postgres
        purge_orphan_migrations
        # Apps loaded in iframes fetch /api/v1/sdk.js — without dist/sdk.js the
        # engine serves a stub that lacks lucidos.ui/data, breaking SDK e2e tests.
        build_sdk
        BUILD="1"
        build_or_find_engine
        swap_ports
        start_engine
    fi

    # ── Vite ──
    detect_vite_tls

    if curl -sk "${VITE_PROTO}://localhost:${vite_port}/" >/dev/null 2>&1; then
        echo "Vite already running on port $vite_port"
    else
        INTERNAL_VITE_PORT="$vite_port"
        ENGINE_PORT="$engine_port"
        start_vite
    fi

    # Final check: engine must proxy frontend (retry up to 30s)
    echo -n "Verifying frontend proxy"
    local proxy_ready=""
    for i in {1..30}; do
        if curl -sk "${PROTO}://localhost:${engine_port}/" 2>/dev/null | grep -q "<!DOCTYPE" 2>/dev/null; then
            echo " ready!"
            proxy_ready="yes"
            break
        fi
        echo -n "."
        sleep 1
    done

    if [ -z "$proxy_ready" ]; then
        echo ""
        echo "WARNING: Engine not serving frontend (Vite proxy may not be connected)"
    fi

    # Export for test scripts
    export VITE_PORT="$engine_port"
}

cleanup_e2e_worktrees() {
    echo "Cleaning up e2e worktrees..."
    local original_dir="$PWD"
    cd "$E2E_WORKSPACE" || return

    # Prune stale worktree entries (paths that no longer exist on disk)
    git worktree prune 2>/dev/null

    # Remove all non-main worktrees (created by CC tests)
    local removed=0
    while IFS= read -r line; do
        local wt_path
        wt_path=$(echo "$line" | awk '{print $1}')
        # Skip the main working tree
        [ "$wt_path" = "$E2E_WORKSPACE" ] && continue
        git worktree remove --force "$wt_path" 2>/dev/null && removed=$((removed + 1))
    done < <(git worktree list 2>/dev/null)

    # Clean up leftover e2e-test branches
    git branch --list 'e2e-test/*' 'claude-code/*' 'merge-tmp/*' 2>/dev/null | xargs -r git branch -D 2>/dev/null

    # CC test worktrees are physically inside this workspace but registered in
    # the lucidos repo (where `git worktree add` was run). Without this the
    # lucidos repo accumulates stale entries every test run; engine recovery
    # then iterates over hundreds of dead worktrees on next startup and
    # exceeds its 30s API readiness budget.
    cd "$_E2E_PROJECT_DIR" 2>/dev/null || { cd "$original_dir"; return; }
    git worktree prune 2>/dev/null
    while IFS= read -r line; do
        local wt_path
        wt_path=$(echo "$line" | awk '{print $1}')
        case "$wt_path" in
            "$E2E_WORKSPACE"/*) git worktree remove --force "$wt_path" 2>/dev/null && removed=$((removed + 1)) ;;
        esac
    done < <(git worktree list 2>/dev/null)
    # Delete claude-code/* branches whose tip is already an ancestor of main
    # (no unique work). Active CC sessions have commits ahead of main and
    # branches checked out by other worktrees aren't deletable, so this is safe.
    git for-each-ref --format='%(refname:short)' refs/heads/claude-code/ 2>/dev/null | while IFS= read -r br; do
        if git merge-base --is-ancestor "$br" main 2>/dev/null; then
            git branch -D "$br" 2>/dev/null || true
        fi
    done

    cd "$original_dir"
    [ "$removed" -gt 0 ] && echo "Removed $removed worktree(s)" || true
}

stop_e2e_workspace() {
    echo "Stopping e2e workspace..."
    "$_E2E_SCRIPTS_DIR/stop.sh" -w "$E2E_WORKSPACE" 2>/dev/null || true

    # Also stop Vite
    if [ -f "$FRONTEND_PIDFILE" ]; then
        local vite_pid
        vite_pid="$(cat "$FRONTEND_PIDFILE" 2>/dev/null)"
        if [ -n "$vite_pid" ] && kill -0 "$vite_pid" 2>/dev/null; then
            kill "$vite_pid" 2>/dev/null || true
        fi
        rm -f "$FRONTEND_PIDFILE"
    fi
}

# Drops the public schema if any row in _sqlx_migrations references a version
# whose .sql file no longer exists in the source. CC branches that get abandoned
# without merging leave orphan migrations in the e2e DB; sqlx::Migrator then
# refuses to start the engine with VersionMissing(...).
purge_orphan_migrations() {
    local migrations_dir="$_E2E_PROJECT_DIR/crates/lucidos-engine/migrations"
    local container="lucidos-pg-$PG_NAME"

    local valid_versions
    valid_versions=$(ls "$migrations_dir" 2>/dev/null | grep -oE '^[0-9]{14}' | sort -u | paste -sd, -)
    [ -z "$valid_versions" ] && return 0

    # Checking first, separately, because Postgres parses both branches of a
    # CASE expression even when one is unreachable — a single combined query
    # errors out on a fresh DB. Two round trips lets the count query fail
    # loudly on a real psql/container problem instead of being masked.
    local table_exists
    table_exists=$(docker exec "$container" psql -U lucidos -d lucidos -At -c \
        "SELECT to_regclass('_sqlx_migrations') IS NOT NULL;")
    [ "$table_exists" = "t" ] || return 0

    local orphan_count
    orphan_count=$(docker exec "$container" psql -U lucidos -d lucidos -At -c \
        "SELECT count(*) FROM _sqlx_migrations WHERE version NOT IN ($valid_versions);")

    if [ "${orphan_count:-0}" -gt 0 ]; then
        echo "Found $orphan_count orphan migration(s) from abandoned branches — resetting schema"
        docker exec "$container" psql -U lucidos -d lucidos -q -c \
            "DROP SCHEMA public CASCADE; CREATE SCHEMA public; CREATE EXTENSION IF NOT EXISTS vector;"
    fi
}

reset_e2e_database() {
    local container="lucidos-pg-$PG_NAME"

    echo "Resetting database..."
    docker exec "$container" psql -U lucidos -q -c "
        DO \$\$
        DECLARE r RECORD;
        BEGIN
            FOR r IN SELECT tablename FROM pg_tables
                WHERE schemaname = 'public' AND tablename != '_sqlx_migrations'
            LOOP
                EXECUTE 'TRUNCATE TABLE ' || quote_ident(r.tablename) || ' CASCADE';
            END LOOP;
        END \$\$;
    "
    echo "Database reset complete"
}
