#!/bin/bash
# Build CognOS engine Docker image
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

# Parse arguments
LOCAL=""
NO_CACHE=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --local) LOCAL="1"; shift ;;
        --no-cache) NO_CACHE="--no-cache"; shift ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --local      Build locally with cargo (not Docker)"
            echo "  --no-cache   Build Docker image without cache"
            echo "  -h, --help   Show this help"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [ -n "$LOCAL" ]; then
    echo "Building CognOS engine locally..."
    cargo build -p cognos-engine -p cognos-cli --release
    echo ""
    echo "Build complete!"
    echo "Binaries at: target/release/cognos-engine, target/release/cognos"
else
    echo "Building CognOS Docker image..."
    docker-compose build $NO_CACHE
    echo ""
    echo "Build complete!"
    echo "Run with: ./scripts/start.sh -w <workspace>"
fi
