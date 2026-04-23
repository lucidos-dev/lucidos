#!/bin/bash
# Initialize a workspace: create dirs, allocate ports, start PostgreSQL.
# Does NOT start engine or Vite. Used by backup restore to provision
# a new workspace from the engine.
#
# Usage: ./scripts/init-workspace.sh -w <name>
# Output (stdout): key=value lines — DATABASE_URL, PG_PORT, API_PORT, VITE_PORT, WORKSPACE
# Progress/logs go to stderr.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
FRONTEND_DIR="$PROJECT_DIR/crates/cognos-app"
SCRIPT_NAME="init-workspace.sh"

source "$SCRIPT_DIR/lib/ports.sh"
source "$SCRIPT_DIR/lib/workspace.sh"

WORKSPACE=""
while [[ $# -gt 0 ]]; do
    case $1 in
        -w|--workspace) WORKSPACE="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: $SCRIPT_NAME -w <workspace>" >&2
            echo "Initialize workspace dirs, allocate ports, start PostgreSQL." >&2
            exit 0
            ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

if [ -z "$WORKSPACE" ]; then
    echo "Error: No workspace specified. Usage: $SCRIPT_NAME -w <name>" >&2
    exit 1
fi

# Redirect all function output to stderr so stdout is clean for key=value output
{
    resolve_workspace
    allocate_ports "$WORKSPACE"
    setup_postgres
} >&2

echo "DATABASE_URL=postgres://cognos:cognos@localhost:$PG_PORT/cognos"
echo "PG_PORT=$PG_PORT"
echo "API_PORT=$API_PORT"
echo "VITE_PORT=$VITE_PORT"
echo "WORKSPACE=$WORKSPACE"
