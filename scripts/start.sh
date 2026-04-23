#!/bin/bash
# Start CognOS engine in Docker
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
WORKSPACE_FILE="$PROJECT_DIR/.cognos-workspace"

cd "$PROJECT_DIR"

# Parse arguments
WORKSPACE="${COGNOS_WORKSPACE:-}"
BUILD=""
FOREGROUND=""
while [[ $# -gt 0 ]]; do
    case $1 in
        -w|--workspace) WORKSPACE="$2"; shift 2 ;;
        -b|--build) BUILD="1"; shift ;;
        -f|--foreground) FOREGROUND="1"; shift ;;
        -h|--help)
            echo "Usage: $0 -w <workspace> [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  -w, --workspace DIR   Workspace directory (required first time)"
            echo "  -b, --build           Rebuild Docker image before starting"
            echo "  -f, --foreground      Run in foreground (see logs)"
            echo "  -h, --help            Show this help"
            echo ""
            echo "Environment:"
            echo "  COGNOS_WORKSPACE              Workspace directory"
            echo "  VERTEX_PROJECT_ID             GCP project ID"
            echo "  VERTEX_REGION                 GCP region (default: europe-west1)"
            echo "  GOOGLE_APPLICATION_CREDENTIALS  Path to GCP credentials"
            echo ""
            echo "Example:"
            echo "  $0 -w ~/.cognos/workspaces/personal"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# Check if workspace is specified, fall back to saved workspace
if [ -z "$WORKSPACE" ]; then
    if [ -f "$WORKSPACE_FILE" ]; then
        WORKSPACE="$(cat "$WORKSPACE_FILE")"
        echo "Using saved workspace: $WORKSPACE"
    else
        echo "Error: No workspace specified."
        echo ""
        echo "Usage: $0 -w <workspace>"
        echo ""
        echo "Example:"
        echo "  $0 -w ~/.cognos/workspaces/personal"
        exit 1
    fi
fi

# Resolve to absolute path and save for next time
WORKSPACE="$(cd "$WORKSPACE" 2>/dev/null && pwd || mkdir -p "$WORKSPACE" && cd "$WORKSPACE" && pwd)"
echo "$WORKSPACE" > "$WORKSPACE_FILE"

# Check if already running
if docker-compose ps --services --filter "status=running" 2>/dev/null | grep -q cognos; then
    echo "CognOS engine already running in Docker"
    echo "Use ./scripts/stop.sh to stop it first"
    exit 1
fi

# Export for docker-compose
export COGNOS_WORKSPACE="$WORKSPACE"
export VERTEX_PROJECT_ID="${VERTEX_PROJECT_ID:-}"
export VERTEX_REGION="${VERTEX_REGION:-europe-west1}"

# Check for GCP credentials
if [ -z "$GOOGLE_APPLICATION_CREDENTIALS" ]; then
    # Try default locations
    if [ -f "$HOME/.config/gcloud/application_default_credentials.json" ]; then
        export GOOGLE_APPLICATION_CREDENTIALS="$HOME/.config/gcloud/application_default_credentials.json"
    else
        echo "Warning: GOOGLE_APPLICATION_CREDENTIALS not set"
        echo "Run: gcloud auth application-default login"
    fi
fi

echo "Starting CognOS engine..."
echo "  Workspace: $WORKSPACE"
echo "  Project:   ${VERTEX_PROJECT_ID:-<not set>}"
echo "  Region:    $VERTEX_REGION"
echo ""

# Build if requested
BUILD_FLAG=""
if [ -n "$BUILD" ]; then
    BUILD_FLAG="--build"
fi

if [ -n "$FOREGROUND" ]; then
    # Run in foreground
    docker-compose up $BUILD_FLAG
else
    # Run in background
    docker-compose up -d $BUILD_FLAG

    # Wait for server to be ready
    echo "Waiting for server..."
    for i in {1..60}; do
        if curl -s http://localhost:3000/health >/dev/null 2>&1; then
            echo ""
            echo "CognOS engine started"
            echo "  API: http://localhost:3000"
            echo "  Logs: docker-compose logs -f"
            echo ""
            echo "Use ./scripts/stop.sh to stop"
            exit 0
        fi
        sleep 1
        printf "."
    done

    echo ""
    echo "Timeout waiting for server. Check logs:"
    docker-compose logs --tail=50
    exit 1
fi
