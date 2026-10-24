#!/usr/bin/env bash
#
# deps-state.sh: what the checkout's npm dependencies are, and where they live.
#
#   ./scripts/deps-state.sh fingerprint         # cksum of every package.json + the lock
#   ./scripts/deps-state.sh stamp-path          # where the last install's fingerprint is kept
#   ./scripts/deps-state.sh install-root        # the npm workspaces root
#
# The build-watch (crates/lucidos-app/dev-build-watch.mjs) is the caller. It has
# to answer the questions `ensure_npm_deps` answers at startup, and there must
# be exactly ONE definition of each: a second cksum written in
# JavaScript over the same file set is a fingerprint that drifts silently, and
# drift here means either a build that never heals or an install that never
# stops. So this is a thin shell wrapper over `scripts/lib/workspace.sh`, not a
# reimplementation.
#
# Why the build-watch needs them at all: a coding agent's Apply lands a new
# `package-lock.json` on main, `ensure_npm_deps` refuses to install while the
# build-watch runs, and every `vite build` afterwards fails
# to resolve the new import. Full account:
# `docs/plans/2026-08-21-a-wedged-frontend-build-heals-itself-and-shouts.md`.
#
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND_DIR="$PROJECT_DIR/crates/lucidos-app"
export PROJECT_DIR FRONTEND_DIR

# shellcheck source=scripts/lib/workspace.sh
source "$PROJECT_DIR/scripts/lib/workspace.sh"

install_root() { _resolve_npm_install_root "$FRONTEND_DIR"; }

usage() {
    echo "usage: $(basename "$0") {fingerprint|stamp-path|install-root}" >&2
    exit 2
}

case "${1:-}" in
    fingerprint)
        _deps_fingerprint "$(install_root)"
        ;;
    stamp-path)
        echo "$(install_root)/node_modules/.lucidos-deps-stamp"
        ;;
    install-root)
        install_root
        ;;
    *)
        usage
        ;;
esac
