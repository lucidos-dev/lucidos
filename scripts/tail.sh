#!/bin/bash
# Tail the Lucidos engine log for a workspace.
#
# Usage:
#   tail.sh myws                   # recent log lines from ~/workspaces/myws
#   tail.sh test -f                 # follow live
#   tail.sh myws -g error           # filter for error lines
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

source "$SCRIPT_DIR/lib/workspace.sh"

WORKSPACE=""
LINES=100
FOLLOW=0
GREP=""

while [[ $# -gt 0 ]]; do
    case $1 in
        -n|--lines)  LINES="$2"; shift 2 ;;
        -g|--grep)   GREP="$2"; shift 2 ;;
        -f|--follow) FOLLOW=1; shift ;;
        -h|--help)
            echo "Usage: $0 <workspace> [OPTIONS]"
            echo ""
            echo "Tail the Lucidos engine log."
            echo ""
            echo "  <workspace> can be a name (resolved via ~/workspaces/<name>),"
            echo "  an absolute path, or a relative path."
            echo ""
            echo "Options:"
            echo "  -n, --lines N       Lines to show (default: 100)"
            echo "  -g, --grep PATTERN  Filter lines (e.g. 'Engine', 'error')"
            echo "  -f, --follow        Follow the log (tail -f)"
            echo "  -h, --help          Show this help"
            exit 0
            ;;
        -*)  echo "Unknown option: $1"; exit 1 ;;
        *)   WORKSPACE="$1"; shift ;;
    esac
done

if [ -z "$WORKSPACE" ]; then
    WORKSPACE="${LUCIDOS_WORKSPACE:-}"
fi
if [ -z "$WORKSPACE" ]; then
    echo "Usage: $0 <workspace> [OPTIONS]"
    echo "  e.g. $0 dev"
    exit 1
fi

resolve_workspace_path

ENGINE_LOG="$WORKSPACE/.lucidos/engine.log"

if [ ! -f "$ENGINE_LOG" ]; then
    echo "No engine log at: $ENGINE_LOG"
    exit 1
fi

if [ -n "$GREP" ]; then
    if [ "$FOLLOW" -eq 1 ]; then
        exec tail -n "$LINES" -f "$ENGINE_LOG" | grep --color=auto --line-buffered -i "$GREP"
    else
        tail -n "$LINES" "$ENGINE_LOG" | grep --color=auto -i "$GREP"
    fi
else
    if [ "$FOLLOW" -eq 1 ]; then
        exec tail -n "$LINES" -f "$ENGINE_LOG"
    else
        tail -n "$LINES" "$ENGINE_LOG"
    fi
fi
