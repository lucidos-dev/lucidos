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
APP_BIN="$APP/Contents/MacOS/Lucidos"
[ -x "$APP_BIN" ] || fail "bundle is missing its executable at $APP_BIN"
log "using app bundle: $APP"

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
# Scrub the dev workspace's Postgres/Lucidos env so the inherited CC-session
# values can't poison the packaged EMBEDDED path (spawn_gateway sets the gateway
# env itself; DATABASE_URL / PG* would otherwise leak into the spawned engine).
# Override only HOME (isolation) and LUCIDOS_ENGINE_PORT (the free port).
env -u DATABASE_URL -u PGHOST -u PGPORT -u PGUSER -u PGPASSWORD -u PGDATABASE \
    -u LUCIDOS_WORKSPACE -u LUCIDOS_STATIC_DIR -u LUCIDOS_SDK_DIR \
    -u LUCIDOS_GATEWAY_DATA -u LUCIDOS_API_PORT -u LUCIDOS_ENGINE_BIN \
    -u LUCIDOS_HOST_PID -u LUCIDOS_FRONTEND_PID \
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

PASSED=1
# cleanup (EXIT trap) stops the service, verifies a clean shutdown, and reports.
