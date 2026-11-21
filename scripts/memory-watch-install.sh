#!/bin/bash
# Install, remove or inspect the host memory watch's launchd agent (macOS).
#
# Usage:
#   ./scripts/memory-watch-install.sh install     load the agent for THIS checkout
#   ./scripts/memory-watch-install.sh uninstall   unload it and remove its plist
#   ./scripts/memory-watch-install.sh status      is it installed, loaded, logging?
#
# Run it from your own checkout, never from a coding-agent worktree: the agent
# keeps pointing at the checkout it was installed from. What the watch does is
# in scripts/lib/memory_watch.sh.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
AGENTS_DIR="$HOME/Library/LaunchAgents"
# shellcheck source=lib/memory_watch.sh
source "$SCRIPT_DIR/lib/memory_watch.sh"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "The memory watch is a launchd agent, so it runs on macOS only." >&2
    exit 1
fi

case "${1:-}" in
    install) memory_watch_install "$PROJECT_DIR" "$AGENTS_DIR" ;;
    uninstall) memory_watch_uninstall "$AGENTS_DIR" ;;
    status) memory_watch_status "$AGENTS_DIR" ;;
    *)
        echo "Usage: $0 install|uninstall|status" >&2
        exit 2
        ;;
esac
