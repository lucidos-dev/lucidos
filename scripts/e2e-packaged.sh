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
# Instead it runs the bundle's headless service role — `Lucidos --service`, which
# spawns the gateway with the full embedded env and never touches
# AppKit/Tauri/notifications/updater/tray/launchd — and asserts the chain over
# HTTP + on disk.
#
# Isolation: the service resolves its data dir from $HOME
# ($HOME/Library/Application Support/com.lucidos.app), so this runs under a temp
# HOME — the embedded cluster, workspaces, and logs are fully isolated from any
# real install and removed on teardown. The service role does NOT install
# launchd (that's the GUI client), so running the inner binary directly with
# --service pollutes nothing.
#
# macOS-only + heavy: building the .app is a full release engine+gateway build +
# a relocatable PostgreSQL download + a frontend build + `cargo tauri build`. So
# this is a STANDALONE script, NOT part of the default ./scripts/e2e.sh run; the
# nightly opts in via `./scripts/e2e.sh --packaged` (or LUCIDOS_E2E_PACKAGED=1).
#
# Usage:
#   ./scripts/e2e-packaged.sh            # reuse an existing .app, else build it
#   ./scripts/e2e-packaged.sh --rebuild  # force a fresh build-dmg.sh build first
#
# Env knobs:
#   FASTEMBED_CACHE_DIR  seed source for the engine's embedding-model cache
#                        (default ${XDG_CACHE_HOME:-$HOME/.cache}/lucidos/fastembed);
#                        symlinked into the temp app-data so a seeded run is
#                        offline/fast and a cold run downloads the model once.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

REBUILD=0
while [ $# -gt 0 ]; do
    case "$1" in
        --rebuild) REBUILD=1; shift ;;
        -h|--help) sed -n '2,40p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "[e2e-packaged] unknown argument: $1" >&2; exit 1 ;;
    esac
done

log()  { echo "[e2e-packaged] $*"; }
fail() { echo "[e2e-packaged] FAIL: $*" >&2; exit 1; }

# ── macOS-only: skip gracefully elsewhere (never red a Linux/CI run) ──────────
if [ "$(uname -s)" != "Darwin" ]; then
    log "SKIP: the packaged build smoke test is macOS-only (uname=$(uname -s))"
    exit 0
fi

# ── Cheap pre-flight: the staged resource contract (no build) ─────────────────
"$SCRIPT_DIR/build-dmg.sh" --check || fail "bundle resource contract check failed"

# ── Ensure a built .app exists (build via build-dmg.sh if missing/forced) ─────
BUNDLE_MACOS="$PROJECT_DIR/target/release/bundle/macos"
find_app() { /usr/bin/find "$BUNDLE_MACOS" -maxdepth 1 -name '*.app' 2>/dev/null | head -1; }
APP="$(find_app)"
if [ "$REBUILD" = "1" ] || [ -z "$APP" ]; then
    log "building the .app via build-dmg.sh (heavy: release build + Postgres fetch + tauri build)…"
    "$SCRIPT_DIR/build-dmg.sh" || fail "build-dmg.sh failed"
    APP="$(find_app)"
fi
[ -n "$APP" ] || fail "no .app found under $BUNDLE_MACOS"
# The main binary's name depends on the tauri-cli version (productName vs the
# crate name, e.g. `Lucidos` vs `lucidos-app`) — resolve it from the bundle's
# own Info.plist instead of hardcoding one convention.
APP_EXEC="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist" 2>/dev/null || true)"
[ -n "$APP_EXEC" ] || fail "could not read CFBundleExecutable from $APP/Contents/Info.plist"
APP_BIN="$APP/Contents/MacOS/$APP_EXEC"
[ -x "$APP_BIN" ] || fail "bundle is missing its executable at $APP_BIN"
log "using app bundle: $APP (executable: $APP_EXEC)"

# ── Isolated temp HOME + free port + seeded fastembed cache ───────────────────
TMP_HOME="$(mktemp -d -t lucidos-pkg-e2e)"
APP_DATA="$TMP_HOME/Library/Application Support/com.lucidos.app"
mkdir -p "$APP_DATA"

# Seed the embedding-model cache so engine warmup is offline/instant when the
# host already has it (the embedder e2e seeds the same dir). spawn_gateway pins
# FASTEMBED_CACHE_DIR to <app-data>/fastembed, so symlink that to the shared
# cache. A cold host still works — it downloads the model once into the shared
# cache, warming future runs.
SHARED_FASTEMBED="${FASTEMBED_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/lucidos/fastembed}"
mkdir -p "$SHARED_FASTEMBED"
ln -s "$SHARED_FASTEMBED" "$APP_DATA/fastembed"

# A free ephemeral port, deliberately clear of 5251 (dev gateway) and 5252
# (packaged gateway) so this coexists with a running dev or installed Lucidos.
PORT=""
for p in $(seq 5300 5399); do
    if ! lsof -ti :"$p" -sTCP:LISTEN >/dev/null 2>&1; then PORT="$p"; break; fi
done
[ -n "$PORT" ] || fail "no free port found in 5300-5399"
BASE="http://localhost:$PORT"
SVC_LOG="$TMP_HOME/service.log"
log "temp HOME: $TMP_HOME"
log "gateway port: $PORT"

# ── Teardown: stop ONLY our captured service PID, verify a clean stop, wipe ────
SVC_PID=""
PASSED=0
cleanup() {
    local ec=$?
    trap - EXIT INT TERM
    if [ -n "$SVC_PID" ] && kill -0 "$SVC_PID" 2>/dev/null; then
        log "stopping service (PID $SVC_PID) with SIGTERM…"
        # SIGTERM → desktop.rs SERVICE_STOP → graceful shutdown of the gateway,
        # its workspace engines, and the embedded Postgres (pg_ctl -m fast).
        kill -TERM "$SVC_PID" 2>/dev/null || true
        for _ in $(seq 1 60); do            # up to 30s
            kill -0 "$SVC_PID" 2>/dev/null || break
            sleep 0.5
        done
        if kill -0 "$SVC_PID" 2>/dev/null; then
            log "service did not stop on SIGTERM within 30s — SIGKILL"
            kill -KILL "$SVC_PID" 2>/dev/null || true
            [ "$PASSED" = "1" ] && ec=1
        fi
    fi
    # On a passing run, teardown must leave no orphaned gateway port and no
    # running embedded postmaster (pg_ctl removes postmaster.pid on a clean stop).
    if [ "$PASSED" = "1" ]; then
        if lsof -ti :"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
            log "teardown left port $PORT bound — gateway did not stop cleanly"; ec=1
        fi
        if [ -f "$APP_DATA/pgdata/postmaster.pid" ]; then
            log "teardown left a postmaster.pid — embedded Postgres did not stop cleanly"; ec=1
        fi
    fi
    # Remove the fastembed symlink explicitly first, then the temp tree. (rm -rf
    # unlinks the symlink rather than recursing into the shared cache, but be
    # explicit so the shared cache is never at risk.)
    rm -f "$APP_DATA/fastembed" 2>/dev/null || true
    rm -rf "$TMP_HOME" 2>/dev/null || true
    if [ "$PASSED" = "1" ] && [ "$ec" = "0" ]; then
        log "PASS: packaged build booted end-to-end and shut down cleanly"
    fi
    exit "$ec"
}
trap cleanup EXIT INT TERM

# ── Launch the bundle's headless service role ─────────────────────────────────
# Scrub EVERY inherited Lucidos/Postgres env var so a dev-workspace or
# CC-session environment can't poison the packaged posture. This must be a
# prefix scrub, not an enumerated list: the earlier explicit -u list missed
# LUCIDOS_TLS_CERT/KEY + LUCIDOS_GATEWAY_ENGINE_LOOPBACK (exported by the dev
# engine into its subprocesses, incl. the nightly trigger's shell), which made
# the packaged gateway serve HTTPS on the test port while the assertions
# curl'ed http — "gateway health never returned 200". Override only HOME
# (isolation) and LUCIDOS_ENGINE_PORT (the free port).
SCRUB_ENV=()
while IFS='=' read -r _name _; do
    case "$_name" in
        LUCIDOS_*|DATABASE_URL|PGHOST|PGPORT|PGUSER|PGPASSWORD|PGDATABASE)
            SCRUB_ENV+=(-u "$_name") ;;
    esac
done < <(env)
# ${arr[@]+"${arr[@]}"} — macOS ships bash 3.2, where expanding an EMPTY
# array under `set -u` is an "unbound variable" error (a clean terminal with
# no LUCIDOS_*/PG* vars hits exactly that); the +-expansion idiom skips it.
env ${SCRUB_ENV[@]+"${SCRUB_ENV[@]}"} \
    HOME="$TMP_HOME" LUCIDOS_ENGINE_PORT="$PORT" \
    "$APP_BIN" --service >"$SVC_LOG" 2>&1 &
SVC_PID=$!
log "launched service PID $SVC_PID (logs: $SVC_LOG)"

# Small helpers. JSON parsing uses python3 (already required by build-dmg.sh).
http_status() { curl -s -o /dev/null -w '%{http_code}' "$1"; }
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
HJSON="$(curl -s "$BASE/~/api/v1/health")"
printf '%s' "$HJSON" | tr -d ' \t\n' | grep -q '"role":"gateway"' \
    || fail "gateway health JSON missing role=gateway: $HJSON"
log "PASS: gateway healthy ($HJSON)"

# ── 2. Picker shell ───────────────────────────────────────────────────────────
[ "$(http_status "$BASE/~/")" = "200" ] || fail "picker (/~/) did not return 200"
curl -s -D - -o /dev/null "$BASE/~/" | grep -iq '^content-type:[[:space:]]*text/html' \
    || fail "picker (/~/) is not text/html"
log "PASS: picker served"

# ── 3. Create a workspace (triggers embedded PG provision + engine spawn) ─────
CREATE="$(curl -s -X POST "$BASE/~/api/v1/control/workspaces" \
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
    LIST="$(curl -s "$BASE/~/api/v1/control/workspaces")"
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

# ── 6. The workspace app shell is served (engine stamps the slug base href) ──
SHELL_HTML="$(curl -s "$BASE/$SLUG/")"
printf '%s' "$SHELL_HTML" | tr -d ' ' | grep -q "basehref=\"/$SLUG/\"" \
    || fail "workspace shell (/$SLUG/) missing <base href=\"/$SLUG/\">"
log "PASS: workspace app shell served with slug base href"

# ── 7. Embedded Postgres cluster exists on disk ───────────────────────────────
[ -f "$APP_DATA/pgdata/PG_VERSION" ] \
    || fail "embedded Postgres cluster not provisioned (no pgdata/PG_VERSION)"
log "PASS: embedded Postgres cluster on disk"

# ── 8. Notification/app-shell serving chain through the gateway ───────────────
# These are the packaged surfaces dev e2e never exercises from a staged bundle:
# a mis-staged Resources/{frontend,sdk} regresses silently otherwise.

# 8a. The JS SDK is served and is NOT the fallback stub (a mis-staged sdk/
# makes sdk.rs serve a console.error stub — apps lose window.lucidos.*).
SDK_JS="$(curl -s "$BASE/$SLUG/api/v1/sdk.js")"
printf '%s' "$SDK_JS" | grep -q 'lucidos' \
    || fail "sdk.js not served through the gateway"
printf '%s' "$SDK_JS" | grep -qi 'SDK bundle not found' \
    && fail "sdk.js is the fallback STUB — Resources/sdk mis-staged"
log "PASS: sdk.js served (not the stub)"

# 8b. The service worker is served as JS with a stamped (non-placeholder)
# BUILD_ID — an unstamped or missing sw.js kills PWA install + web push.
SW_STATUS="$(http_status "$BASE/$SLUG/sw.js")"
[ "$SW_STATUS" = "200" ] || fail "sw.js did not return 200 (got $SW_STATUS)"
curl -s -D - -o /dev/null "$BASE/$SLUG/sw.js" | grep -iq '^content-type:.*javascript' \
    || fail "sw.js served with a non-JS content type (SW registration would fail)"
SW_JS="$(curl -s "$BASE/$SLUG/sw.js")"
printf '%s' "$SW_JS" | grep -q "BUILD_ID" \
    || fail "sw.js has no BUILD_ID marker at all"
printf '%s' "$SW_JS" | grep -q "__LUCIDOS_BUILD_ID__" \
    && fail "sw.js BUILD_ID is the unstamped placeholder — frontend staged from an unstamped build"
log "PASS: sw.js served as JS with a stamped BUILD_ID"

# 8c. The PWA manifest is served (404 breaks installability on every device).
[ "$(http_status "$BASE/$SLUG/manifest.json")" = "200" ] \
    || fail "manifest.json did not return 200"
log "PASS: manifest.json served"

# 8d. The web-push VAPID key endpoint answers with a key (push subscription
# bootstrap for browser/PWA clients over the Tailscale https path).
VAPID="$(curl -s "$BASE/$SLUG/api/v1/push/vapid-key")"
printf '%s' "$VAPID" | tr -d ' \t\n' | grep -q '"public_key":"' \
    || fail "push/vapid-key missing public_key: $VAPID"
log "PASS: VAPID public key served"

# 8e. The engine-shipped system-knowhow reference set is staged (Resources/
# system-knowhow) AND resolved by the engine (LUCIDOS_SYSTEM_KNOWHOW_DIR). A
# mis-staged or unresolved dir makes load_knowhow('system-knowhow/…') and
# GET /api/v1/knowhow silently degrade — the exact packaged regression this
# resource was added to fix.
[ -f "$APP/Contents/Resources/system-knowhow/glossary.md" ] \
    || fail "Resources/system-knowhow/glossary.md missing — system-knowhow not staged into the bundle"
KNOWHOW="$(curl -s "$BASE/$SLUG/api/v1/knowhow")"
printf '%s' "$KNOWHOW" | grep -q 'system-knowhow/' \
    || fail "GET /api/v1/knowhow lists no system-knowhow/ id — LUCIDOS_SYSTEM_KNOWHOW_DIR unresolved in the packaged engine"
log "PASS: system-knowhow staged + resolved (Resources/system-knowhow + /api/v1/knowhow)"

PASSED=1
# cleanup (EXIT trap) stops the service, verifies a clean shutdown, and reports.
