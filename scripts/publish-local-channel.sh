#!/usr/bin/env bash
# Stage a signed candidate for the gateway's Local update channel.
# Agents discover the directory through their own engine; omit --port there.
# Usage: scripts/publish-local-channel.sh <version> [--port N] [--staging-dir DIR]
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/local_channel.sh
source "$SCRIPT_DIR/lib/local_channel.sh"
VERSION=""
STAGING=""
PORT=""
usage() { echo 'Usage: publish-local-channel.sh <version> [--port N] [--staging-dir DIR]' >&2; exit 2; }
while [ "$#" -gt 0 ]; do
    case "$1" in
        --port) [ "$#" -ge 2 ] || usage; PORT="$2"; shift 2 ;;
        --staging-dir) [ "$#" -ge 2 ] || usage; STAGING="$2"; shift 2 ;;
        -*) usage ;;
        *) [ -z "$VERSION" ] || usage; VERSION="$1"; shift ;;
    esac
done
[ -n "$VERSION" ] || usage
if [ -z "$STAGING" ]; then
    RELEASE_ROOT="$(dirname "$(git -C "$SCRIPT_DIR" rev-parse --path-format=absolute --git-common-dir)")"
    STAGING="$RELEASE_ROOT/.lucidos/release-worktrees/$VERSION/.lucidos/release-staging/$VERSION"
fi
local_channel_publish "$STAGING" "$VERSION" "$PORT"
printf 'Staged v%s for Local. Select it with:\n    lucidos update-channel set local\nReturn with:\n    lucidos update-channel set stable\n' "$VERSION"
