#!/bin/bash
# Build Lucidos engine Docker image
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
    echo "Building Lucidos engine locally..."
    cargo build -p lucidos-engine -p lucidos-cli --release
    echo ""
    echo "Build complete!"
    echo "Binaries at: target/release/lucidos-engine, target/release/lucidos"
else
    echo "Building Lucidos Docker image..."
    docker-compose build $NO_CACHE
    echo ""
    echo "Build complete!"
    echo "Run with: ./scripts/start.sh -w <workspace>"
fi
