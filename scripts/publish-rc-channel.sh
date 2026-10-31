#!/usr/bin/env bash
# publish-rc-channel.sh: copy a staged release candidate into a dev workspace's
# rc channel, so a packaged app can update to it before it is published.
#
#   ./scripts/publish-rc-channel.sh <version> [--workspace DIR] [--staging-dir DIR]
#
# The workspace defaults to $LUCIDOS_WORKSPACE, and its engine must be running
# on plain http. The staging defaults to the one release.sh --verify-build left.
# Then set the app's update address to the URL this prints. Steps:
# docs/desktop-app.md, "Test an update before publishing".
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/rc_channel.sh
source "$SCRIPT_DIR/lib/rc_channel.sh"

usage() {
    sed -n 5p "$0" | sed 's/^#  *//' >&2
    exit 2
}

VERSION=""
WORKSPACE="${LUCIDOS_WORKSPACE:-}"
STAGING=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --workspace) [ "$#" -ge 2 ] || usage; WORKSPACE="$2"; shift 2 ;;
        --staging-dir) [ "$#" -ge 2 ] || usage; STAGING="$2"; shift 2 ;;
        -h|--help) usage ;;
        -*) echo "ERROR: unknown option $1" >&2; usage ;;
        *) [ -z "$VERSION" ] || usage; VERSION="$1"; shift ;;
    esac
done
[ -n "$VERSION" ] || usage
[ -n "$WORKSPACE" ] || {
    echo "ERROR: no workspace. Pass --workspace DIR or set LUCIDOS_WORKSPACE." >&2
    exit 2
}

if [ -z "$STAGING" ]; then
    # Release worktrees live in the main checkout, also when this runs from a
    # coding-agent worktree.
    MAIN_ROOT="$(dirname "$(git -C "$SCRIPT_DIR" rev-parse --path-format=absolute --git-common-dir)")"
    STAGING="$MAIN_ROOT/.lucidos/release-worktrees/$VERSION/.lucidos/release-staging/$VERSION"
fi

URL="$(rc_channel_publish "$STAGING" "$WORKSPACE" "$VERSION")"
cat <<EOF
Published v$VERSION to the rc channel in $WORKSPACE.
Point the app at it, then use Check for Updates in Settings:
    lucidos update-address set $URL
Go back to the published channel afterwards:
    lucidos update-address reset
EOF
