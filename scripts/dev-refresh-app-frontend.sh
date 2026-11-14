#!/usr/bin/env bash
# Fast FRONTEND-ONLY iteration loop for the PACKAGED macOS app (`Lucidos.app`).
#
# Why this exists: native macOS notifications (and anything else that needs a
# bundle identifier) are inert in `tauri dev` — `UNUserNotificationCenter`
# throws for an unbundled `cargo run` binary, so `notifications::{setup,show}`
# short-circuit on `tauri::is_dev()`. The native path therefore only runs in a
# real `.app`, and a *runnable* one needs `./scripts/build-dmg.sh` (it stages the
# engine/gateway/Postgres/frontend resources via `--config`; a bare `cargo tauri
# build` omits them). That full build is heavy.
#
# But the packaged app serves its UI over HTTP from
# `<App>/Contents/Resources/frontend/` (the bundled `dist/`), spawned by the
# bundled engine which reads those files from disk per request. So a
# FRONTEND-only change (e.g. native-notification deep-link routing in
# `crates/lucidos-app/src/store/actions/`) can be tested WITHOUT re-running
# `build-dmg.sh`: rebuild `dist/`, mirror it into the installed bundle, re-seal
# the bundle signature (changing Resources invalidates the outer CodeResources
# seal), then reload the window.
#
# NOT covered: the Rust half (the `notifications.rs` delegate / `show`, the Tauri
# commands, anything compiled INTO the `.app`). Change Rust → full
# `./scripts/build-dmg.sh`. This script only refreshes the served frontend.
#
# Usage:
#   ./scripts/dev-refresh-app-frontend.sh [-a <app path>] [--no-build] [--restart]
#
#   -a, --app <path>   Installed bundle to refresh (default: /Applications/Lucidos.app)
#   --no-build         Skip `npm run build`; sync the existing crates/lucidos-app/dist
#   --restart          Also kickstart the always-on gateway service (forces the
#                      engine to re-serve from a clean state). Off by default —
#                      a window reload (Cmd-R) is enough since the engine serves
#                      the swapped files from disk; use this only if a reload
#                      doesn't pick up the change.
#   -h, --help         Show this help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

APP="/Applications/Lucidos.app"
DO_BUILD=1
DO_RESTART=0

die() {
    echo "❌ $*" >&2
    exit 1
}

# macOS denies writes into an app bundle in /Applications ("Operation not
# permitted" / EPERM) unless the calling terminal holds App Management or Full
# Disk Access — a privacy gate, NOT a POSIX perms problem (which would be
# "Permission denied"). Print the fix and the no-permission alternative.
perm_hint() {
    cat >&2 <<EOF
❌ Can't modify $APP — macOS denied the write ("Operation not permitted").
   This is a privacy gate, not a file-permission problem. Fix (pick one):

   • Grant your terminal Full Disk Access (most reliable) or App Management:
       System Settings → Privacy & Security → Full Disk Access → enable your
       terminal app (Terminal / iTerm / Ghostty / VS Code …), then fully quit
       and reopen it and re-run this script.

   • Or skip in-place editing — full rebuild + reinstall (no privacy grant):
       ./scripts/build-dmg.sh
       then drag the new Lucidos.app over /Applications in Finder (Finder holds
       the entitlement, so the copy isn't subject to this gate).
EOF
}

usage() {
    cat <<'EOF'
Fast frontend-only refresh for the packaged macOS app (Lucidos.app).

Rebuilds the frontend, syncs it into the installed bundle's
Contents/Resources/frontend (served live by the bundled engine), and re-seals
the bundle signature. Use for frontend-only changes (e.g. notification
deep-link routing); a Rust change still needs ./scripts/build-dmg.sh.

Usage:
  ./scripts/dev-refresh-app-frontend.sh [-a <app path>] [--no-build] [--restart]

  -a, --app <path>   Installed bundle (default: /Applications/Lucidos.app)
  --no-build         Skip `npm run build`; sync the existing crates/lucidos-app/dist
  --restart          Also kickstart the always-on gateway service (forces a clean
                     re-serve). Off by default — a window reload (Cmd-R) usually
                     suffices since the engine serves the swapped files from disk.
  -h, --help         Show this help
EOF
    exit "${1:-0}"
}

while [ $# -gt 0 ]; do
    case "$1" in
        -a|--app) APP="${2:?--app needs a path}"; shift 2 ;;
        --no-build) DO_BUILD=0; shift ;;
        --restart) DO_RESTART=1; shift ;;
        -h|--help) usage 0 ;;
        *) echo "Unknown argument: $1" >&2; usage 1 ;;
    esac
done

[ "$(uname -s)" = "Darwin" ] || die "macOS only (the packaged .app + UserNotifications path are macOS-only)."

RES="$APP/Contents/Resources"
FRONTEND="$RES/frontend"
[ -d "$APP" ] || die "App not found: $APP — pass -a <path> to your installed Lucidos.app."
[ -d "$FRONTEND" ] || die "Bundled frontend not found: $FRONTEND — is $APP a build-dmg.sh bundle?"

# Writability pre-check, BEFORE the (slow) build: every later step writes into
# the bundle (stage frontend.new, swap, re-sign), and all fail under the App
# Management / Full Disk Access gate. Probe the dir where frontend.new is staged
# so the fix is reported instantly instead of after a wasted rebuild.
_probe="$RES/.lucidos-write-probe.$$"
if ! (touch "$_probe" && rm -f "$_probe") 2>/dev/null; then
    perm_hint
    exit 1
fi

DIST="$REPO_ROOT/crates/lucidos-app/dist"

if [ "$DO_BUILD" = 1 ]; then
    echo "🔨 Building frontend (npm run build)…"
    (cd "$REPO_ROOT/crates/lucidos-app" && npm run build)
fi
[ -f "$DIST/index.html" ] || die "No $DIST/index.html — build failed, or --no-build with an empty dist."

# Mirror dist → bundle frontend. Stage into a sibling then swap so an interrupted
# copy can't leave the served frontend half-written (the engine serves it live).
echo "📦 Syncing dist → $FRONTEND"
rm -rf "${FRONTEND}.new"
ditto "$DIST" "${FRONTEND}.new" || { perm_hint; exit 1; }
rm -rf "${FRONTEND}.old"
mv "$FRONTEND" "${FRONTEND}.old"
mv "${FRONTEND}.new" "$FRONTEND"
rm -rf "${FRONTEND}.old"

# Changing Resources invalidates the bundle's outer CodeResources seal, so macOS
# refuses to (re)launch the modified app until it's re-sealed. Only the outer
# bundle changed (frontend files are not code), so a shallow re-sign suffices —
# the nested Mach-O signatures (engine, gateway, helpers) are untouched. Prefer
# the stable dev identity (so TCC grants persist) and fall back to ad-hoc.
# shellcheck source=scripts/lib/codesign.sh
source "$SCRIPT_DIR/lib/codesign.sh"
if lucidos_signing_identity_ready; then
    lucidos_ensure_keychain_in_search_list
    security unlock-keychain -p "$LUCIDOS_SIGNING_KC_PASS" "$LUCIDOS_SIGNING_KEYCHAIN" 2>/dev/null || true
    if codesign --force --keychain "$LUCIDOS_SIGNING_KEYCHAIN" \
        --sign "$LUCIDOS_SIGNING_IDENTITY" "$APP" 2>/dev/null; then
        echo "🔏 Re-sealed bundle with dev identity ($LUCIDOS_SIGNING_IDENTITY)."
    else
        echo "⚠️  Dev-identity re-sign failed; falling back to ad-hoc."
        codesign --force --sign - "$APP" >/dev/null 2>&1 || die "Re-sign failed; macOS may refuse to launch $APP."
        echo "🔏 Re-sealed bundle ad-hoc."
    fi
else
    codesign --force --sign - "$APP" >/dev/null 2>&1 || die "Re-sign failed; macOS may refuse to launch $APP."
    echo "🔏 Re-sealed bundle ad-hoc (run ./scripts/dev-codesign-setup.sh once for stable TCC grants)."
fi

if [ "$DO_RESTART" = 1 ]; then
    TARGET="gui/$(id -u)/com.lucidos.engine"
    if launchctl kickstart -k "$TARGET" 2>/dev/null; then
        echo "🔄 Restarted gateway service ($TARGET)."
    else
        echo "ℹ️  Could not kickstart $TARGET (not loaded?). Open the app once, or skip --restart."
    fi
fi

echo "✅ Frontend refreshed. Reload the app window (Cmd-R) to pick it up."
echo "   If a stale service worker pins the old build, fully quit and reopen the app."
