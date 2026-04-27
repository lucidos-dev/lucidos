#!/bin/bash
# View Lucidos engine Docker logs
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

# Parse arguments
FOLLOW=""
LINES="100"
while [[ $# -gt 0 ]]; do
    case $1 in
        -f|--follow) FOLLOW="-f"; shift ;;
        -n|--lines) LINES="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  -f, --follow      Follow log output"
            echo "  -n, --lines NUM   Number of lines to show (default: 100)"
            echo "  -h, --help        Show this help"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

docker-compose logs --tail="$LINES" $FOLLOW
