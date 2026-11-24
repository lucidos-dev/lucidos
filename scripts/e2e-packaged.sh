#!/usr/bin/env bash
#
# e2e-packaged.sh — boot smoke test for the PACKAGED macOS desktop build.
#
# Proves the thing dev e2e (direct engine + Docker Postgres) never exercises:
# the packaged chain actually boots end-to-end — staged Resources, the bundled
# gateway + engine binaries, relocatable EMBEDDED Postgres provisioning, a
# per-workspace database, the engine spawn, and static serving through the
# gateway proxy.
#
# It does NOT drive the WKWebView UI: Apple's WKWebView exposes no WebDriver and
# tauri-driver supports only Linux/Windows, so the packaged window cannot be
# automated on macOS (see docs/adr/0016-packaged-tauri-e2e-boot-smoke-test.md).
# Runs `Lucidos --disposable-service <new-root> <port>` without the UI or
# launchd. Fresh storage, a bounded supervisor and owned process groups isolate
# it from live installations (ADR 0702); the persistent worktree guard stays on.
#
# macOS Apple Silicon only, heavy and standalone. Every invocation rebuilds
# this checkout's Developer ID-signed package, then checks normal shutdown,
# cancellation and abrupt controller loss against the actual bundled runtime.
#
# Usage:
#   ./scripts/e2e-packaged.sh
#   ./scripts/e2e-packaged.sh --rebuild  # accepted for compatibility; always builds
#
# FASTEMBED_CACHE_DIR optionally seeds an owned COPY after gateway startup.
# A cold cache downloads into the disposable scope; host caches are never changed.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
# shellcheck source=scripts/lib/loopback_curl.sh
source "$SCRIPT_DIR/lib/loopback_curl.sh"

while [ $# -gt 0 ]; do
    case "$1" in
        --rebuild) shift ;;
        -h|--help) sed -n '2,40p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "[e2e-packaged] unknown argument: $1" >&2; exit 1 ;;
    esac
done

log()  { echo "[e2e-packaged] $*"; }
fail() { echo "[e2e-packaged] FAIL: $*" >&2; exit 1; }
# Consume all input: grep -q can SIGPIPE a large producer under pipefail.
has_match() { grep "$@" >/dev/null; }

# An explicitly requested packaged smoke must never succeed without running.
if [ "$(uname -s)" != "Darwin" ]; then
    fail "the packaged build smoke requires macOS (uname=$(uname -s))"
fi

# Build each invocation: an arbitrary existing .app is not source provenance.
[ -n "${APPLE_SIGNING_IDENTITY:-}" ] \
    || fail "building this smoke requires APPLE_SIGNING_IDENTITY for a Developer ID Application certificate"
"$SCRIPT_DIR/build-dmg.sh" --check || fail "bundle resource contract check failed"
log "building the .app via build-dmg.sh (heavy: release build + Postgres fetch + tauri build)…"
"$SCRIPT_DIR/build-dmg.sh" || fail "build-dmg.sh failed"
BUNDLE_MACOS="$PROJECT_DIR/target/release/bundle/macos"
APP="$(/usr/bin/find "$BUNDLE_MACOS" -maxdepth 1 -name '*.app' 2>/dev/null | head -1)"
[ -n "$APP" ] || fail "no .app found under $BUNDLE_MACOS"
# The main binary's name depends on the tauri-cli version (productName vs the
# crate name, e.g. `Lucidos` vs `lucidos-app`) — resolve it from the bundle's
# own Info.plist instead of hardcoding one convention.
APP_EXEC="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist" 2>/dev/null || true)"
[ -n "$APP_EXEC" ] || fail "could not read CFBundleExecutable from $APP/Contents/Info.plist"
APP_BIN="$APP/Contents/MacOS/$APP_EXEC"
[ -x "$APP_BIN" ] || fail "bundle is missing its executable at $APP_BIN"
# The app names its data dir after its bundle identifier, so read that too.
APP_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist" 2>/dev/null || true)"
[ -n "$APP_ID" ] || fail "could not read CFBundleIdentifier from $APP/Contents/Info.plist"
log "using app bundle: $APP (executable: $APP_EXEC, identifier: $APP_ID)"

# Inspect the engine the service will actually spawn, not a dev build.
ENGINE_BIN="$APP/Contents/Resources/lucidos-engine"
codesign --verify --strict "$APP" || fail "app signature verification failed"
codesign --verify --strict "$ENGINE_BIN" || fail "bundled engine signature verification failed"
ENGINE_SIGNATURE="$(codesign -dv --verbose=4 "$ENGINE_BIN" 2>&1)" \
    || fail "cannot inspect engine signature"
printf '%s' "$ENGINE_SIGNATURE" | has_match 'flags=.*runtime' \
    || fail "bundled engine lacks hardened runtime signing"
printf '%s' "$ENGINE_SIGNATURE" | has_match '^Authority=Developer ID Application:' \
    || fail "bundled engine is not signed with a Developer ID Application identity"
# Current codesign defaults to an abstract dump. Older versions lack --xml;
# their colon-prefixed output path requests the same header-free plist XML.
ENGINE_ENTITLEMENTS="$(codesign -d --xml --entitlements - "$ENGINE_BIN" 2>/dev/null)" \
    || ENGINE_ENTITLEMENTS="$(codesign -d --entitlements :- "$ENGINE_BIN" 2>/dev/null)" \
    || fail "cannot inspect engine entitlements"
printf '%s' "$ENGINE_ENTITLEMENTS" | python3 -c '
import plistlib, sys
entitlements = plistlib.loads(sys.stdin.buffer.read())
sys.exit(0 if entitlements.get("com.apple.security.cs.allow-unsigned-executable-memory") is True else 1)
' \
    || fail "bundled engine lacks its compiled-code execution entitlement"

# Short paths keep bundled Postgres's Unix socket within macOS's limit.
TMP_PARENT="$(mktemp -d /tmp/lucidos-pkg.XXXXXX)" || fail "cannot create test parent"
SCOPE="$TMP_PARENT/normal" # MUST NOT exist: the app creates and claims fresh storage.
APP_DATA="$SCOPE/gateway"
SHARED_FASTEMBED="${FASTEMBED_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/lucidos/fastembed}"
PORT=""
for p in $(seq 5300 5399); do
    if ! lsof -ti :"$p" -sTCP:LISTEN >/dev/null 2>&1; then PORT="$p"; break; fi
done
[ -n "$PORT" ] || fail "no free port found in 5300-5399"
BASE="$(loopback_origin "$PORT")"
SVC_LOG="$TMP_PARENT/normal.log"
LIFECYCLE="$SCRIPT_DIR/lib/stage_runtime_packaged_lifecycle.py"
SVC_PID=""
PASSED=0

record_owned() {
    python3 "$LIFECYCLE" snapshot "$SCOPE" "$PORT" > "$TMP_PARENT/owned.json"
}
stop_owned() {
    local mode="$1" status=0 captured=1 stop_failed=0
    record_owned || captured=0
    if [ -n "$SVC_PID" ]; then
        case "$mode" in
            kill) kill -KILL "$SVC_PID" 2>/dev/null || return 1 ;;
            *) kill -TERM "$SVC_PID" 2>/dev/null || true ;;
        esac
        # A killed controller is expected to fail; a graceful stop must succeed.
        wait "$SVC_PID" || status=$?
        SVC_PID=""
        if [ "$mode" != kill ] && [ "$status" != 0 ]; then
            log "controller failed during graceful stop ($status)"
            stop_failed=1
        fi
    fi
    [ "$captured" = 1 ] || return 1
    python3 "$LIFECYCLE" verify "$SCOPE" "$TMP_PARENT/owned.json" || return 1
    return "$stop_failed"
}
cleanup() {
    local ec=$?
    trap - EXIT INT TERM
    if [ -n "$SVC_PID" ]; then stop_owned normal || ec=1; fi
    if [ "$PASSED" != 1 ] || [ "$ec" != 0 ]; then
        # Keep the full scope, including live-process evidence, on ANY failure.
        log "failure diagnostics and disposable storage retained: $TMP_PARENT"
        mkdir -p "$PROJECT_DIR/.lucidos"
        cp "$SVC_LOG" "$PROJECT_DIR/.lucidos/packaged-smoke-service.log" 2>/dev/null || true
    else
        rm -rf "$TMP_PARENT"
        log "PASS: signed packaged smoke and all lifecycle checks"
    fi
    exit "$ec"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
launch() {
    # The app also clears its descendants' environment. Clear the controller's
    # environment too: no host account, proxy or service configuration enters.
    # Preserve an existing outer sandbox fact; macOS cannot nest seatbelts.
    local sandbox_env=()
    if [ "${LUCIDOS_AGENT_SANDBOXED:-}" = 1 ]; then
        sandbox_env+=(LUCIDOS_AGENT_SANDBOXED=1)
    fi
    env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin ${sandbox_env[@]+"${sandbox_env[@]}"} \
        "$APP_BIN" --disposable-service "$SCOPE" "$PORT" >"$SVC_LOG" 2>&1 &
    SVC_PID=$!
    log "launched disposable package (logs: $SVC_LOG)"
}
seed_cache() {
    # Only after gateway claims the fresh scope. Dereference links when copying;
    # never point a test runtime into host storage, even on a warm host.
    if [ -d "$SHARED_FASTEMBED" ]; then
        mkdir -p "$APP_DATA/fastembed" || fail "cannot create disposable embedding cache"
        cp -RL "$SHARED_FASTEMBED/." "$APP_DATA/fastembed/" \
            || fail "cannot copy embedding cache into disposable scope"
    fi
}
launch

# Small helpers. JSON parsing uses python3 (already required by build-dmg.sh).
#
# Gateway control auth and the loopback CA are minted inside this scope, never
# under the host HOME. The disposable service relocates its private subtree to
# $SCOPE/private, so the CA copy sits beside it.
SCOPE_CA="$(LUCIDOS_PRIVATE_DIR="$SCOPE/private" loopback_ca_cert)"
gateway_curl() {
    local token_file="$SCOPE/private/local-token"
    if [ -r "$token_file" ]; then
        curl --noproxy '*' --cacert "$SCOPE_CA" --max-time 60 -H "x-lucidos-local-token: $(cat "$token_file")" "$@"
    else
        curl --noproxy '*' --cacert "$SCOPE_CA" --max-time 60 "$@"
    fi
}
http_status() { gateway_curl -s -o /dev/null -w '%{http_code}' "$1"; }
alive() { kill -0 "$SVC_PID" 2>/dev/null; }
dump_log() { echo "----- service log (tail) -----"; tail -40 "$SVC_LOG" 2>/dev/null; echo "------------------------------"; }

# ── 1. Gateway health (binds immediately; embedded PG comes up lazily) ────────
log "waiting for gateway health at $BASE/~/api/v1/health …"
ok=0
for _ in $(seq 1 60); do                    # up to 60s
    alive || { dump_log; fail "service process died during gateway boot"; }
    if [ "$(http_status "$BASE/~/api/v1/health")" = "200" ]; then ok=1; break; fi
    sleep 1
done
[ "$ok" = "1" ] || { dump_log; fail "gateway health never returned 200"; }
HJSON="$(gateway_curl -s "$BASE/~/api/v1/health")"
printf '%s' "$HJSON" | tr -d ' \t\n' | has_match '"role":"gateway"' \
    || fail "gateway health JSON missing role=gateway: $HJSON"
log "PASS: gateway healthy ($HJSON)"
seed_cache

# ── 2. Picker shell ───────────────────────────────────────────────────────────
[ "$(http_status "$BASE/~/")" = "200" ] || fail "picker (/~/) did not return 200"
gateway_curl -s -D - -o /dev/null "$BASE/~/" | has_match -i '^content-type:[[:space:]]*text/html' \
    || fail "picker (/~/) is not text/html"
log "PASS: picker served"

# ── 3. Create a workspace (triggers embedded PG provision + engine spawn) ─────
CREATE="$(gateway_curl -s -X POST "$BASE/~/api/v1/control/workspaces" \
    -H 'Content-Type: application/json' -d '{"name":"smoke"}')"
SLUG="$(printf '%s' "$CREATE" | python3 -c \
    'import sys,json; print(json.load(sys.stdin)["workspace"]["id"])' 2>/dev/null)"
[ -n "$SLUG" ] || { dump_log; fail "could not create workspace (response: $CREATE)"; }
log "created workspace slug: $SLUG"

# ── 4. Poll the workspace to healthy (the load-bearing assertion) ─────────────
# Cold boot = embedded cluster initdb + DB create + migrations + warmup + engine
# bind, so allow a generous window.
log "waiting for workspace '$SLUG' to become healthy …"
ok=0
for _ in $(seq 1 120); do                   # up to 240s
    alive || { dump_log; fail "service process died while the workspace was booting"; }
    LIST="$(gateway_curl -s "$BASE/~/api/v1/control/workspaces")"
    H="$(printf '%s' "$LIST" | SLUG="$SLUG" python3 -c \
        'import sys,json,os; s=os.environ["SLUG"]; d=json.load(sys.stdin); print(next((w.get("health","") for w in d.get("workspaces",[]) if w.get("id")==s), ""))' \
        2>/dev/null)"
    [ "$H" = "healthy" ] && { ok=1; break; }
    [ "$H" = "unhealthy" ] && { dump_log; fail "workspace '$SLUG' became unhealthy"; }
    sleep 2
done
[ "$ok" = "1" ] || { dump_log; fail "workspace '$SLUG' did not become healthy within the timeout"; }
log "PASS: workspace '$SLUG' healthy"

# ── 5. Reach the workspace engine THROUGH the gateway ─────────────────────────
[ "$(http_status "$BASE/$SLUG/api/v1/health")" = "200" ] \
    || fail "engine health via gateway (/$SLUG/api/v1/health) did not return 200"
log "PASS: engine reachable through gateway"
ENGINE_HEALTH="$(gateway_curl -s "$BASE/$SLUG/api/v1/health")"
printf '%s' "$ENGINE_HEALTH" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("packaged") is True else 1)' \
    || fail "bundled engine exposed the surrounding source checkout"
log "PASS: bundle remains packaged inside the coding-agent checkout"
REPOSITORIES="$(gateway_curl -s "$BASE/$SLUG/api/v1/repositories")"
printf '%s' "$REPOSITORIES" | SCOPE="$SCOPE" python3 -c '
import json, os, pathlib, sys
root = pathlib.Path(os.environ["SCOPE"]).resolve()
sys.exit(0 if all(pathlib.Path(repo["path"]).resolve().is_relative_to(root) for repo in json.load(sys.stdin)) else 1)
' || fail "disposable engine registered a repository outside its scope"
log "PASS: registered repositories stay inside disposable storage"

# ── 6. The workspace app shell is served (engine stamps the slug base href) ──
SHELL_HTML="$(gateway_curl -s "$BASE/$SLUG/")"
printf '%s' "$SHELL_HTML" > "$TMP_PARENT/workspace-shell.html"
printf '%s' "$SHELL_HTML" | tr -d ' ' | has_match "basehref=\"/$SLUG/\"" \
    || fail "workspace shell (/$SLUG/) missing <base href=\"/$SLUG/\">"
log "PASS: workspace app shell served with slug base href"

# ── 7. Embedded Postgres cluster exists on disk ───────────────────────────────
[ -f "$APP_DATA/pgdata/PG_VERSION" ] \
    || fail "embedded Postgres cluster not provisioned (no pgdata/PG_VERSION)"
log "PASS: embedded Postgres cluster on disk"

# ── 7b. The workspace publishes .lucidos/ports, so it is addressable by name ──
# The only thing `lucidos spawn-thread --to <ws>` and the engine's
# workspace_resolver read. Nothing wrote it on a packaged install until ADR 0136,
# so every cross-workspace spawn failed with "no .lucidos/ports". PROTO matters
# just as much: a packaged engine serves plain http behind the gateway, and a
# missing line defaults the resolver to https, which dies with "record overflow".
PORTS_FILE="$APP_DATA/workspaces/$SLUG/.lucidos/ports"
[ -f "$PORTS_FILE" ] \
    || fail "workspace '$SLUG' published no .lucidos/ports (cross-workspace spawns cannot resolve it)"
has_match -x "API_PORT=[0-9][0-9]*" "$PORTS_FILE" \
    || fail "$PORTS_FILE has no API_PORT= line: $(cat "$PORTS_FILE")"
has_match -x "PROTO=http" "$PORTS_FILE" \
    || fail "a packaged engine serves http, but $PORTS_FILE says: $(cat "$PORTS_FILE")"
log "PASS: workspace published .lucidos/ports with API_PORT + PROTO=http"

# ── 8. Notification/app-shell serving chain through the gateway ───────────────
# These are the packaged surfaces dev e2e never exercises from a staged bundle:
# a mis-staged Resources/{frontend,sdk} regresses silently otherwise.

# 8a. The JS SDK is served and is NOT the fallback stub (a mis-staged sdk/
# makes sdk.rs serve a console.error stub — apps lose window.lucidos.*).
SDK_JS="$(gateway_curl -s "$BASE/$SLUG/api/v1/sdk.js")"
printf '%s' "$SDK_JS" | has_match 'lucidos' \
    || fail "sdk.js not served through the gateway"
printf '%s' "$SDK_JS" | has_match -i 'SDK bundle not found' \
    && fail "sdk.js is the fallback STUB — Resources/sdk mis-staged"
log "PASS: sdk.js served (not the stub)"

# 8b. The service worker is served as JS with a stamped (non-placeholder)
# BUILD_ID — an unstamped or missing sw.js kills PWA install + web push.
SW_STATUS="$(http_status "$BASE/$SLUG/sw.js")"
[ "$SW_STATUS" = "200" ] || fail "sw.js did not return 200 (got $SW_STATUS)"
gateway_curl -s -D - -o /dev/null "$BASE/$SLUG/sw.js" | has_match -i '^content-type:.*javascript' \
    || fail "sw.js served with a non-JS content type (SW registration would fail)"
SW_JS="$(gateway_curl -s "$BASE/$SLUG/sw.js")"
printf '%s' "$SW_JS" | has_match "BUILD_ID" \
    || fail "sw.js has no BUILD_ID marker at all"
printf '%s' "$SW_JS" | has_match "__LUCIDOS_BUILD_ID__" \
    && fail "sw.js BUILD_ID is the unstamped placeholder — frontend staged from an unstamped build"
log "PASS: sw.js served as JS with a stamped BUILD_ID"

# 8c. The PWA manifest is served (404 breaks installability on every device).
[ "$(http_status "$BASE/$SLUG/manifest.json")" = "200" ] \
    || fail "manifest.json did not return 200"
log "PASS: manifest.json served"

# 8d. The web-push VAPID key endpoint answers with a key (push subscription
# bootstrap for browser/PWA clients over the Tailscale https path).
VAPID="$(gateway_curl -s "$BASE/$SLUG/api/v1/push/vapid-key")"
printf '%s' "$VAPID" | tr -d ' \t\n' | has_match '"public_key":"' \
    || fail "push/vapid-key missing public_key: $VAPID"
log "PASS: VAPID public key served"

# 8e. The engine-shipped system-knowhow reference set is staged (Resources/
# system-knowhow) AND resolved by the engine (LUCIDOS_SYSTEM_KNOWHOW_DIR). A
# mis-staged or unresolved dir makes load_knowhow('system-knowhow/…') and
# GET /api/v1/knowhow silently degrade — the exact packaged regression this
# resource was added to fix.
[ -f "$APP/Contents/Resources/system-knowhow/glossary.md" ] \
    || fail "Resources/system-knowhow/glossary.md missing — system-knowhow not staged into the bundle"
KNOWHOW="$(gateway_curl -s "$BASE/$SLUG/api/v1/knowhow")"
printf '%s' "$KNOWHOW" | has_match 'system-knowhow/' \
    || fail "GET /api/v1/knowhow lists no system-knowhow/ id — LUCIDOS_SYSTEM_KNOWHOW_DIR unresolved in the packaged engine"
log "PASS: system-knowhow staged + resolved (Resources/system-knowhow + /api/v1/knowhow)"

# 8f. The built-in widgets are staged AND resolved (LUCIDOS_SYSTEM_WIDGETS_DIR),
# the same pair of checks for the same reason (ADR 0415).
[ -f "$APP/Contents/Resources/system-widgets/lucidos-sound-player/manifest.json" ] \
    || fail "Resources/system-widgets/lucidos-sound-player missing: system-widgets not staged into the bundle"
PLAYER="$(gateway_curl -s "$BASE/$SLUG/api/v1/app?id=lucidos-sound-player")"
printf '%s' "$PLAYER" | has_match '"built_in":true' \
    || fail "GET /api/v1/app?id=lucidos-sound-player is not the built-in: LUCIDOS_SYSTEM_WIDGETS_DIR unresolved in the packaged engine ($PLAYER)"
log "PASS: system-widgets staged + resolved (Resources/system-widgets + /api/v1/app)"

# Both cases share this service and workspace. The fixture owns only a local
# fake upstream; it never boots an engine or reaches an external account.
TOKEN_FILE="$SCOPE/private/local-token"
ROUND_TRIPS_PASSED=1
python3 "$SCRIPT_DIR/lib/stage_runtime_packaged_smoke.py" "$BASE/$SLUG" "$TOKEN_FILE" "$SCOPE_CA" \
    || { dump_log; ROUND_TRIPS_PASSED=0; }

stop_owned normal || fail "normal package teardown failed"
log "PASS: normal package shutdown"

# Each case provisions REAL bundled Postgres and an engine in a new scope.
# TERM models task cancellation; KILL proves cleanup after abrupt controller loss.
for mode in cancel kill; do
    SCOPE="$TMP_PARENT/$mode"
    APP_DATA="$SCOPE/gateway"
    SVC_LOG="$TMP_PARENT/$mode.log"
    launch
    ok=0
    for _ in $(seq 1 60); do
        alive || { dump_log; fail "$mode controller died during boot"; }
        if [ "$(http_status "$BASE/~/api/v1/health")" = 200 ]; then ok=1; break; fi
        sleep 1
    done
    [ "$ok" = 1 ] || fail "$mode gateway boot timed out"
    seed_cache
    CREATE="$(gateway_curl -s -X POST "$BASE/~/api/v1/control/workspaces" \
        -H 'Content-Type: application/json' -d '{"name":"smoke"}')"
    SLUG="$(printf '%s' "$CREATE" | python3 -c 'import json,sys; print(json.load(sys.stdin)["workspace"]["id"])')"
    [ -n "$SLUG" ] || fail "$mode workspace creation failed: $CREATE"
    ok=0
    for _ in $(seq 1 120); do
        alive || { dump_log; fail "$mode controller died during engine boot"; }
        if [ "$(http_status "$BASE/$SLUG/api/v1/health")" = 200 ]; then ok=1; break; fi
        sleep 2
    done
    [ "$ok" = 1 ] || fail "$mode engine boot timed out"
    [ -f "$APP_DATA/pgdata/postmaster.pid" ] || fail "$mode missing running bundled Postgres"
    stop_owned "$mode" || fail "$mode package cleanup failed"
    log "PASS: $mode released bundled Postgres, engines and gateway"
done
[ "$ROUND_TRIPS_PASSED" = 1 ] \
    || fail "packaged proxy/script smoke failed; diagnose before changing production execution (lifecycle checks passed)"
PASSED=1
