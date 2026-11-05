#!/bin/bash
#
# run.sh — start Lucidos to USE it (end-user / installer entry point).
#
# This is the user-facing launcher: the one the one-click installer (install.sh)
# and an installed end user run. It brings up the SAME stack as the developer
# script (PostgreSQL + the native engine + the lucidos-gateway, with the engine
# serving the built dist/ via LUCIDOS_STATIC_DIR, ADR 0014) by delegating to
# scripts/web-dev.sh — it does NOT duplicate that orchestration. It only flips
# two defaults that make sense for *using* Lucidos rather than developing it:
#
#   • RELEASE engine build by default (slower to build, faster at runtime).
#     web-dev.sh defaults to a debug build for fast iteration; set
#     LUCIDOS_DEBUG_BUILD=1 to fall back to that quicker (debug) build here too.
#   • ONE-SHOT frontend build (a single `vite build`) instead of the developer
#     `vite build --watch` file watcher — an installed user never edits source,
#     so no rebuild-on-change watcher is left running after startup.
#
# Everything else (workspace resolution, ports, Postgres, gateway, exit-after-
# ready) is web-dev.sh's, unchanged. Developers should keep using web-dev.sh —
# this script is intentionally thin so the two stay in lockstep.
#
# Usage:  ./scripts/run.sh -w <workspace> [extra web-dev.sh args]
#   -w/--workspace is required (same contract as web-dev.sh). Extra args are
#   forwarded verbatim (e.g. LUCIDOS_NO_GATEWAY=1 still works via the env).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# One-shot frontend build (no `vite build --watch`). Consumed by start_vite in
# scripts/lib/workspace.sh, which branches to build_frontend_oneshot when set.
export LUCIDOS_FRONTEND_ONESHOT=1

# Build the engine in release mode by default; LUCIDOS_DEBUG_BUILD=1 opts back
# into a faster debug build. -b is always passed so a fresh install builds the
# engine (and a later `git pull` rebuild picks up new source; cargo is
# incremental, so an unchanged tree rebuilds cheaply).
flags=(-b)
if [ -z "${LUCIDOS_DEBUG_BUILD:-}" ]; then
    flags+=(-r)
fi

exec "$SCRIPT_DIR/web-dev.sh" "${flags[@]}" "$@"
