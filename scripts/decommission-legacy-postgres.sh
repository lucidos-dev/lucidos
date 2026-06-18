#!/bin/bash
# Remove a verified legacy per-workspace Postgres container/volume after the
# workspace has been migrated to the shared Postgres cluster.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
FRONTEND_DIR="$PROJECT_DIR/crates/lucidos-app"
SCRIPT_NAME="decommission-legacy-postgres.sh"

cd "$PROJECT_DIR"

WORKSPACE=""
DRY_RUN=""
while [[ $# -gt 0 ]]; do
    case $1 in
        -w|--workspace) WORKSPACE="$2"; shift 2 ;;
        --dry-run) DRY_RUN="1"; shift ;;
        -h|--help)
            echo "Usage: $0 -w <workspace> [--dry-run]"
            echo ""
            echo "Remove the old per-workspace PostgreSQL container/volume only after"
            echo "the shared-cluster migration has been verified."
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [ -z "$WORKSPACE" ]; then
    echo "Error: -w <workspace> is required." >&2
    exit 1
fi

source "$SCRIPT_DIR/lib/ports.sh"
source "$SCRIPT_DIR/lib/workspace.sh"

resolve_workspace_path
PG_NAME=$(printf '%s' "$WORKSPACE" | cksum | awk '{print $1}')
db="$(workspace_database_name)"
marker="$(_shared_pg_migration_marker)"
legacy_container="$(_legacy_pg_container)"
legacy_volume="lucidos-pg-data-$PG_NAME"
shared_container="$(shared_pg_container)"

if [ ! -f "$marker" ]; then
    echo "ERROR: no shared Postgres verification marker found:" >&2
    echo "  $marker" >&2
    echo "Start the workspace once to migrate/verify before decommissioning." >&2
    exit 1
fi

if ! docker inspect "$shared_container" >/dev/null 2>&1; then
    echo "ERROR: shared PostgreSQL container not found: $shared_container" >&2
    exit 1
fi
if [ "$(docker inspect --format='{{.State.Status}}' "$shared_container" 2>/dev/null)" != "running" ]; then
    echo "ERROR: shared PostgreSQL container is not running: $shared_container" >&2
    exit 1
fi
if ! _verify_shared_pg_database "$db"; then
    echo "ERROR: shared database $db is not reachable; refusing to decommission legacy data." >&2
    exit 1
fi

echo "Verified shared database: $shared_container / $db"

if [ -n "$DRY_RUN" ]; then
    echo "Would remove legacy container: $legacy_container"
    echo "Would remove legacy volume:    $legacy_volume"
    exit 0
fi

removed=""
if docker inspect "$legacy_container" >/dev/null 2>&1; then
    echo "Removing legacy PostgreSQL container $legacy_container"
    docker rm -f "$legacy_container" >/dev/null || true
    removed="1"
fi

if docker volume inspect "$legacy_volume" >/dev/null 2>&1; then
    echo "Removing legacy PostgreSQL volume $legacy_volume"
    docker volume rm "$legacy_volume" >/dev/null || true
    removed="1"
fi

if [ -n "$removed" ]; then
    echo "Legacy PostgreSQL decommissioned for $WORKSPACE"
else
    echo "No legacy PostgreSQL container/volume found for $WORKSPACE"
fi
