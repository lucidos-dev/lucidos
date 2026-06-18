#!/usr/bin/env bash
#
# build-dmg.sh — assemble and build the self-contained Lucidos desktop app.
#
# Produces a macOS .app + .dmg whose Resources bundle EVERYTHING the app needs
# at runtime — no Docker, no system Postgres, no dev tools:
#   • the gateway binary           (target/release/lucidos-gateway)
#   • the engine binary            (target/release/lucidos-engine)
#   • the built frontend           (crates/lucidos-app/dist)
#   • the JS SDK bundle            (packages/lucidos-sdk/dist)
#   • a relocatable PostgreSQL 18 + pgvector  (theseus-rs binaries + compiled ext)
#
# The Tauri desktop service (crates/lucidos-app/src/desktop.rs) boots the
# bundled gateway; the gateway provisions bundled Postgres and spawns bundled
# engines per workspace.
#
# MUST run on a Mac (it runs `cargo tauri build` and, optionally, codesign +
# notarytool). It is the buildable half of the ".dmg" deliverable; the
# credentialed half (Apple Developer ID, Tauri updater signing key, GitHub
# Releases publishing) is env-gated below and documented in
# docs/desktop-app.md.
#
# ── Script-vs-LLM contract (release pipeline) ────────────────────────────────
# The deterministic shell pipeline is the SPINE; the LLM/chat layer only drafts
# the changelog, gets approval (the `draft` step), and handles anomalies.
#
# In --release mode this script OWNS the build → codesign → notarize → staple →
# upload-asset stages, and emits its own ReleaseStep* domain events at each stage
# boundary (via scripts/lib/release_events.sh) so the Release Cockpit app lights
# up stage by stage. The cockpit is a PURE READ-ONLY CONSUMER — this script never
# writes to it. Each stage emits ReleaseStepFailed (not a silent exit) on error so
# the cockpit shows red. release.sh / release-to-lucidos.sh own the surrounding
# git / tag / GitHub-Release / changelog stages and the final LucidosReleased.
#
# Credentials in --release mode come ONLY from the auto-injected environment
# (APPLE_ID / APPLE_TEAM_ID are DB env vars; APPLE_PASSWORD /
# TAURI_SIGNING_PRIVATE_KEY are credentials mapped to those names). This script
# never re-exports or overrides them — it asserts each is non-empty at startup and
# fails loud if one is missing (the v0.10.1 clobber bug was an empty
# `export APPLE_ID="$CRED_APPLE_ID"` in the hand-improvised LLM layer).
#
# Usage:
#   ./scripts/build-dmg.sh                 # build an unsigned local .dmg (no events)
#   ./scripts/build-dmg.sh --check         # validate the staged resource contract
#   APPLE_SIGNING_IDENTITY="Developer ID Application: …" \
#   APPLE_ID=… APPLE_PASSWORD=… APPLE_TEAM_ID=… \
#   TAURI_SIGNING_PRIVATE_KEY=… TAURI_SIGNING_PRIVATE_KEY_PASSWORD=… \
#     ./scripts/build-dmg.sh               # signed + notarized (local, no events/upload)
#   ./scripts/build-dmg.sh --release \
#     --release-version <N.N.N> --upload-tag v<N.N.N> \
#     --notes-file <changelog-section> --repo-slug <owner/repo>
#                                          # full release: assert creds, version
#                                          # guard, emit ReleaseStep* events,
#                                          # sign+notarize+staple, upload assets
#
# Release-mode flags:
#   --release            enable release mode (events, asserted creds, asset upload)
#   --release-version V  expected version; must equal the RELEASE file (guard)
#   --upload-tag TAG     GitHub Release tag to attach the DMG + updater assets to
#   --notes-file FILE    CHANGELOG section used as the latest.json `notes`
#   --repo-slug OWNER/R  GitHub repo for the release (default lucidos-dev/lucidos)
#
# Env knobs (all optional):
#   PG_VERSION        relocatable PostgreSQL version       (default 18.4.0)
#   PGVECTOR_VERSION  pgvector tag to compile              (default 0.8.2)
#   TARGET_TRIPLE     theseus triple override              (default: host)

# -E (errtrace) is REQUIRED, not optional: it makes the ERR trap (on_err, below)
# inherit into shell functions, subshells, and command substitutions. Without it,
# an unguarded failure inside sign_app_bundle / refresh_dmg_payload / sign_dmg /
# upload_release_assets would exit via `set -e` WITHOUT firing on_err — so no
# ReleaseStepFailed would be emitted and the cockpit would stall mid-stage, the
# exact "silent stall" this refactor exists to kill.
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_DIR="$REPO_ROOT/crates/lucidos-app"
STAGE="$APP_DIR/bundle-resources"
BUNDLED_EXECUTABLES=(lucidos-engine lucidos-gateway)
RESOURCE_NAMES=(lucidos-engine lucidos-gateway frontend postgres sdk)

PG_VERSION="${PG_VERSION:-18.4.0}"   # match the dev/docker stack (pgvector/pgvector:pg18)
PGVECTOR_VERSION="${PGVECTOR_VERSION:-0.8.2}"

# Shared ReleaseStep* / LucidosReleased emit helpers (the cockpit contract).
# shellcheck source=scripts/lib/release_events.sh
source "$SCRIPT_DIR/lib/release_events.sh"

# Release-mode state (set by arg parsing below). In default (local-build) mode
# RELEASE_MODE stays 0: no events, no asserted creds, no asset upload.
RELEASE_MODE=0
RELEASE_VERSION_ARG=""
UPLOAD_TAG=""
NOTES_FILE=""
REPO_SLUG="lucidos-dev/lucidos"
EFFECTIVE_VERSION=""   # the version stamped into the DMG; set after arg parse
CURRENT_STEP=""        # cockpit step id currently in flight (for failure emit)

step() { printf '\n==> %s\n' "$*"; }

# die emits a ReleaseStepFailed for the in-flight step (release mode only) so the
# cockpit shows red instead of the pipeline silently stopping, then exits.
die() {
    printf 'ERROR: %s\n' "$*" >&2
    trap - ERR
    if [ "$RELEASE_MODE" = "1" ] && [ -n "$CURRENT_STEP" ]; then
        emit_release_step Failed "$CURRENT_STEP" "$EFFECTIVE_VERSION" "$*"
    fi
    exit 1
}

# ERR trap: a stage command failing under `set -e` (not via die) still emits
# ReleaseStepFailed for the in-flight step before the script exits.
on_err() {
    local ec=$?
    trap - ERR
    if [ "$RELEASE_MODE" = "1" ] && [ -n "$CURRENT_STEP" ]; then
        emit_release_step Failed "$CURRENT_STEP" "$EFFECTIVE_VERSION" \
            "Stage '$CURRENT_STEP' failed (exit $ec). See the release log for details."
    fi
    exit "$ec"
}

# begin_step / end_step bracket a deterministic stage. CURRENT_STEP is tracked
# unconditionally (so die/on_err can attribute a failure) but events are emitted
# only in release mode.
begin_step() {  # <step-id> <started-summary>
    CURRENT_STEP="$1"
    if [ "$RELEASE_MODE" = "1" ]; then
        emit_release_step Started "$1" "$EFFECTIVE_VERSION" "$2"
    fi
}
end_step() {    # <step-id> <success-summary>
    if [ "$RELEASE_MODE" = "1" ]; then
        emit_release_step Succeeded "$1" "$EFFECTIVE_VERSION" "$2"
    fi
    CURRENT_STEP=""
}

# Assert the signing/notarization credentials are present. Release mode relies
# ONLY on the auto-injected environment — it never re-exports them — so a missing
# var must fail loud here rather than silently skip notarization (v0.10.1).
assert_release_credentials() {
    local missing=()
    [ -n "${APPLE_SIGNING_IDENTITY:-}" ]   || missing+=("APPLE_SIGNING_IDENTITY")
    [ -n "${APPLE_ID:-}" ]                 || missing+=("APPLE_ID")
    [ -n "${APPLE_PASSWORD:-}" ]           || missing+=("APPLE_PASSWORD")
    [ -n "${APPLE_TEAM_ID:-}" ]            || missing+=("APPLE_TEAM_ID")
    [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || missing+=("TAURI_SIGNING_PRIVATE_KEY")
    if [ "${#missing[@]}" -gt 0 ]; then
        die "release mode requires these auto-injected vars to be non-empty: ${missing[*]} (do NOT export/override them — they are injected as DB env vars + mapped credentials; see docs/desktop-app.md § Shipping)"
    fi
}

resource_config_json() {
    printf '%s' '{"bundle":{"resources":{"bundle-resources/lucidos-engine":"lucidos-engine","bundle-resources/lucidos-gateway":"lucidos-gateway","bundle-resources/frontend":"frontend","bundle-resources/postgres":"postgres","bundle-resources/sdk":"sdk"}}}'
}

# The committed tauri.conf.json pins version 0.1.0; a release must stamp the real
# version so artifacts are named Lucidos_<version>_<arch> and the updater manifest
# matches the tag. Read it from the RELEASE file at the repo root (the release
# worktree carries the bumped value). Empty → caller falls back to tauri.conf.json.
release_version() {
    local f="$REPO_ROOT/RELEASE"
    [ -f "$f" ] || return 0
    tr -d '[:space:]' < "$f"
}

# The full --config payload handed to `cargo tauri build`: the resource map, plus
# a version override when RELEASE is present.
tauri_build_config_json() {
    local ver
    ver="$(release_version)"
    if [ -n "$ver" ]; then
        printf '%s' "{\"version\":\"$ver\",\"bundle\":{\"resources\":{\"bundle-resources/lucidos-engine\":\"lucidos-engine\",\"bundle-resources/lucidos-gateway\":\"lucidos-gateway\",\"bundle-resources/frontend\":\"frontend\",\"bundle-resources/postgres\":\"postgres\",\"bundle-resources/sdk\":\"sdk\"}}}"
    else
        resource_config_json
    fi
}

usage() {
    cat <<'EOF'
Usage:
  ./scripts/build-dmg.sh                 # build an unsigned local .dmg (no events)
  ./scripts/build-dmg.sh --check         # validate the staged resource contract
  APPLE_SIGNING_IDENTITY="Developer ID Application: …" \
  APPLE_ID=… APPLE_PASSWORD=… APPLE_TEAM_ID=… \
  TAURI_SIGNING_PRIVATE_KEY=… TAURI_SIGNING_PRIVATE_KEY_PASSWORD=… \
    ./scripts/build-dmg.sh               # signed + notarized (local, no events/upload)
  ./scripts/build-dmg.sh --release \
    --release-version <N.N.N> --upload-tag v<N.N.N> \
    --notes-file <changelog-section> --repo-slug <owner/repo>
                                         # full release: assert creds, version
                                         # guard, emit ReleaseStep* events,
                                         # sign+notarize+staple, upload assets

Release-mode flags:
  --release            enable release mode (events, asserted creds, asset upload)
  --release-version V  expected version; must equal the RELEASE file (guard)
  --upload-tag TAG     GitHub Release tag to attach the DMG + updater assets to
  --notes-file FILE    CHANGELOG section used as the latest.json `notes`
  --repo-slug OWNER/R  GitHub repo for the release (default lucidos-dev/lucidos)

Env knobs (all optional):
  PG_VERSION        relocatable PostgreSQL version       (default 18.4.0)
  PGVECTOR_VERSION  pgvector tag to compile              (default 0.8.2)
  TARGET_TRIPLE     theseus triple override              (default: host)
EOF
}

check_resource_contract() {
    local cfg expected
    cfg="$(resource_config_json)"
    for expected in "${RESOURCE_NAMES[@]}"; do
        case "$cfg" in
            *"\"bundle-resources/$expected\":\"$expected\""*) ;;
            *) die "Tauri resource map missing bundle-resources/$expected → $expected" ;;
        esac
    done
    for expected in "${BUNDLED_EXECUTABLES[@]}"; do
        case " ${RESOURCE_NAMES[*]} " in
            *" $expected "*) ;;
            *) die "bundled executable $expected is not listed in RESOURCE_NAMES" ;;
        esac
    done
    printf 'OK: build-dmg resources include %s\n' "${RESOURCE_NAMES[*]}"
}

DO_CHECK=0
while [ $# -gt 0 ]; do
    case "$1" in
        --check)           DO_CHECK=1; shift ;;
        -h|--help)         usage; exit 0 ;;
        --release)         RELEASE_MODE=1; shift ;;
        --release-version) [ $# -ge 2 ] || die "--release-version requires an argument"; RELEASE_VERSION_ARG="$2"; shift 2 ;;
        --upload-tag)      [ $# -ge 2 ] || die "--upload-tag requires an argument"; UPLOAD_TAG="$2"; shift 2 ;;
        --notes-file)      [ $# -ge 2 ] || die "--notes-file requires an argument"; NOTES_FILE="$2"; shift 2 ;;
        --repo-slug)       [ $# -ge 2 ] || die "--repo-slug requires an argument"; REPO_SLUG="$2"; shift 2 ;;
        *)                 die "unknown argument: $1" ;;
    esac
done

if [ "$DO_CHECK" = "1" ]; then
    check_resource_contract
    exit 0
fi

# Resolve the version to stamp into the DMG name. In release mode RELEASE is
# authoritative and required; an explicit --release-version must agree with it
# (the up-front half of the version-stamp guard — catches a stale RELEASE before
# we spend a full build producing a misnamed asset). The post-build half asserts
# the produced DMG filename actually carries this version.
EFFECTIVE_VERSION="$(release_version || true)"
if [ "$RELEASE_MODE" = "1" ]; then
    [ -n "$EFFECTIVE_VERSION" ] \
        || die "release mode requires a non-empty RELEASE file at $REPO_ROOT/RELEASE"
    if [ -n "$RELEASE_VERSION_ARG" ] && [ "$RELEASE_VERSION_ARG" != "$EFFECTIVE_VERSION" ]; then
        die "version-stamp mismatch: --release-version '$RELEASE_VERSION_ARG' != RELEASE '$EFFECTIVE_VERSION'. The DMG is named from RELEASE; bump RELEASE to match the release version before building (this is the guard against the v0.10.1 0.10.0-vs-0.10.1 mismatch)."
    fi
fi

# Now that EFFECTIVE_VERSION + RELEASE_MODE are known, arm the failure trap so any
# stage error emits ReleaseStepFailed for the in-flight step.
trap on_err ERR

# Release mode relies solely on auto-injected creds — assert before any work so a
# missing var fails loud instead of silently skipping notarization.
if [ "$RELEASE_MODE" = "1" ]; then
    assert_release_credentials
fi

[ "$(uname -s)" = "Darwin" ] || die "build-dmg.sh builds the macOS bundle and must run on a Mac."

# Resolve the theseus-rs relocatable-binary triple for this host.
case "$(uname -m)" in
    arm64|aarch64) HOST_ARCH="aarch64" ;;
    x86_64)        HOST_ARCH="x86_64" ;;
    *) die "unsupported arch $(uname -m)" ;;
esac
TARGET_TRIPLE="${TARGET_TRIPLE:-${HOST_ARCH}-apple-darwin}"

command -v cargo >/dev/null || die "cargo not found — install Rust (https://rustup.rs)."
if ! cargo tauri --version >/dev/null 2>&1; then
    die "tauri CLI not found. Install it:  cargo install tauri-cli --locked"
fi
command -v npm >/dev/null || die "npm not found — install Node.js."

begin_step build "Compiling engine + gateway + app, fetching PostgreSQL $PG_VERSION + pgvector, running cargo tauri build (.app + .dmg)."

# ── 1. frontend ─────────────────────────────────────────────────────────────
step "Building frontend (dist/)"
(cd "$REPO_ROOT" && npm install)
(cd "$REPO_ROOT/packages/lucidos-sdk" && npm run build)   # /api/v1/sdk.js for app UIs
(cd "$APP_DIR" && npm run build)
[ -f "$APP_DIR/dist/index.html" ] || die "frontend build did not produce dist/index.html"

# ── 2. gateway + engine (release) ───────────────────────────────────────────
step "Building gateway + engine (release)"
# .cargo/config.toml sets rustc-wrapper=sccache; disable it if sccache is absent
# so the build doesn't fail on a missing wrapper.
command -v sccache >/dev/null || export RUSTC_WRAPPER=""
(cd "$REPO_ROOT" && cargo build -p lucidos-engine -p lucidos-gateway -p lucidos-cli --release)
ENGINE_BIN="$REPO_ROOT/target/release/lucidos-engine"
GATEWAY_BIN="$REPO_ROOT/target/release/lucidos-gateway"
[ -x "$ENGINE_BIN" ] || die "engine binary not found at $ENGINE_BIN"
[ -x "$GATEWAY_BIN" ] || die "gateway binary not found at $GATEWAY_BIN"

# ── 3. relocatable PostgreSQL + pgvector ────────────────────────────────────
# Mirrors scripts/prototype/desktop-pg-pgvector-spike.sh (proven recipe).
step "Fetching relocatable PostgreSQL $PG_VERSION + building pgvector $PGVECTOR_VERSION ($TARGET_TRIPLE)"
PG_WORK="$REPO_ROOT/.lucidos/dmg-build/pg"
mkdir -p "$PG_WORK"
PG_DIRNAME="postgresql-${PG_VERSION}-${TARGET_TRIPLE}"
PG_PREFIX="$PG_WORK/$PG_DIRNAME"
PGCONFIG="$PG_PREFIX/bin/pg_config"
if [ ! -x "$PGCONFIG" ]; then
    curl -fsSL -m 300 -o "$PG_WORK/$PG_DIRNAME.tar.gz" \
        "https://github.com/theseus-rs/postgresql-binaries/releases/download/${PG_VERSION}/${PG_DIRNAME}.tar.gz"
    tar -xzf "$PG_WORK/$PG_DIRNAME.tar.gz" -C "$PG_WORK"
fi
[ -x "$PGCONFIG" ] || die "pg_config missing after extract"

SHAREDIR="$("$PGCONFIG" --sharedir)"
if [ ! -f "$SHAREDIR/extension/vector.control" ]; then
    step "Compiling pgvector against the bundled PG"
    curl -fsSL -m 180 -o "$PG_WORK/pgvector.tar.gz" \
        "https://github.com/pgvector/pgvector/archive/refs/tags/v${PGVECTOR_VERSION}.tar.gz"
    rm -rf "$PG_WORK/pgvector" && mkdir -p "$PG_WORK/pgvector"
    tar -xzf "$PG_WORK/pgvector.tar.gz" -C "$PG_WORK/pgvector" --strip-components=1
    # The theseus tarball bakes its CI Xcode SDK path into PGXS; override it.
    ( cd "$PG_WORK/pgvector" \
        && make -s PG_CONFIG="$PGCONFIG" PG_SYSROOT="$(xcrun --show-sdk-path)" \
        && make -s install PG_CONFIG="$PGCONFIG" PG_SYSROOT="$(xcrun --show-sdk-path)" )
fi
[ -f "$SHAREDIR/extension/vector.control" ] || die "pgvector did not install into the bundled PG"

# ── 4. stage resources ──────────────────────────────────────────────────────
step "Staging bundle resources → $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp "$ENGINE_BIN" "$STAGE/lucidos-engine"
cp "$GATEWAY_BIN" "$STAGE/lucidos-gateway"
for bin in "${BUNDLED_EXECUTABLES[@]}"; do
    chmod +x "$STAGE/$bin"
done
cp -R "$APP_DIR/dist" "$STAGE/frontend"
cp -R "$PG_PREFIX" "$STAGE/postgres"
# The JS SDK (/api/v1/sdk.js) used by app-UI iframes; the engine finds it via
# LUCIDOS_SDK_DIR (set by the desktop launcher to <resources>/sdk).
cp -R "$REPO_ROOT/packages/lucidos-sdk/dist" "$STAGE/sdk"

# ── 5. tauri build ──────────────────────────────────────────────────────────
step "Running cargo tauri build (app + dmg)"
# Inject the resource map at build time (kept out of the committed tauri.conf.json
# so normal `cargo check` / dev builds aren't tied to staged artifacts).
RESOURCES_CONFIG="$(tauri_build_config_json)"
TAURI_BUILD_ARGS=(tauri build --bundles app,dmg --config "$RESOURCES_CONFIG")
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    # Tauri signs sidecars/framework-style nested code, but these CLI tools are
    # plain Resources entries. Build unsigned, then sign the exact resource
    # binaries before refreshing the DMG payload below.
    TAURI_BUILD_ARGS+=(--no-sign)
fi
(cd "$APP_DIR" && cargo "${TAURI_BUILD_ARGS[@]}")

BUNDLE_DIR="$REPO_ROOT/target/release/bundle"
DMG_PATH="$(/usr/bin/find "$BUNDLE_DIR/dmg" -name '*.dmg' 2>/dev/null | head -1 || true)"
APP_PATH="$(/usr/bin/find "$BUNDLE_DIR/macos" -name '*.app' 2>/dev/null | head -1 || true)"
[ -n "$DMG_PATH" ] || die "no .dmg produced under $BUNDLE_DIR/dmg"
[ -n "$APP_PATH" ] || die "no .app produced under $BUNDLE_DIR/macos"

# Version-stamp guard (post-build half). Tauri names the artifact
# Lucidos_<version>_<arch>.dmg from the --config version override; assert the
# produced DMG actually carries EFFECTIVE_VERSION so a stamping failure can't ship
# a misnamed asset (the v0.10.1 "DMG built as 0.10.0 while the tag is v0.10.1"
# class of bug). Only enforced when a version is known (always, in release mode).
if [ -n "$EFFECTIVE_VERSION" ]; then
    case "$(basename "$DMG_PATH")" in
        *"_${EFFECTIVE_VERSION}_"*) ;;
        *) die "version-stamp guard: produced DMG '$(basename "$DMG_PATH")' does not carry version '$EFFECTIVE_VERSION'. The build did not stamp the release version into the artifact name." ;;
    esac
fi

end_step build "Built $(basename "$DMG_PATH") (.app + .dmg + updater artifacts)."

# ── 5b. sign (env-gated) ────────────────────────────────────────────────────
sign_app_bundle() {
    local app="$1"
    local resources="$app/Contents/Resources"
    local bin path

    # Sanity-check the explicitly-bundled executables are present before we begin.
    for bin in "${BUNDLED_EXECUTABLES[@]}"; do
        [ -x "$resources/$bin" ] || die "expected bundled executable at $resources/$bin"
    done

    # `codesign --deep` does NOT sign loose Mach-O files (standalone executables
    # and .dylibs that aren't part of a nested bundle/framework) — it only
    # recurses into nested bundles. The relocatable Postgres tree
    # (Contents/Resources/postgres/{bin,lib}/*) is exactly that: ~200 loose
    # Mach-O files. They must each get a Developer ID signature, secure
    # timestamp, and hardened runtime or notarization fails (v0.10.1, submission
    # 37ebf142, statusCode 4000). So we discover every Mach-O file in the bundle
    # and sign them inside-out (leaves first, outer .app last).
    #
    # Walk all regular files and select the Mach-O ones via `file`. `sort -rz`
    # gives a depth-descending order (longest paths — deepest nesting — first),
    # so nested/leaf binaries are always signed before their containers. NUL
    # separation keeps spaces in paths safe.
    local -a macho_files=()
    while IFS= read -r -d '' path; do
        case "$(file -b "$path")" in
            *Mach-O*) macho_files+=("$path") ;;
        esac
    done < <(/usr/bin/find "$app" -type f -print0 | sort -rz)

    [ "${#macho_files[@]}" -gt 0 ] || die "no Mach-O binaries found inside $app"

    step "Signing ${#macho_files[@]} Mach-O binaries inside-out"
    for path in "${macho_files[@]}"; do
        codesign --force --options runtime --timestamp \
            --sign "$APPLE_SIGNING_IDENTITY" "$path" \
            || die "codesign failed for $path"
    done

    # Sign the outer .app LAST. Keep --deep as belt-and-suspenders (re-seals any
    # nested bundle), but the loose payload above is what makes notarization pass.
    codesign --force --deep --options runtime --timestamp \
        --sign "$APPLE_SIGNING_IDENTITY" "$app" \
        || die "codesign failed for $app"

    # Verify the whole bundle, then spot-verify a couple of the Postgres binaries
    # that previously slipped through unsigned.
    codesign --verify --deep --strict --verbose=2 "$app" \
        || die "codesign --verify failed for $app"
    for bin in "${BUNDLED_EXECUTABLES[@]}"; do
        codesign --verify --strict --verbose=2 "$resources/$bin" \
            || die "codesign --verify failed for $resources/$bin"
    done
    for path in "$resources/postgres/bin/postgres" "$resources/postgres/lib/libpq.5.dylib"; do
        if [ -e "$path" ]; then
            codesign --verify --strict --verbose=2 "$path" \
                || die "codesign --verify failed for $path"
        fi
    done
}

sign_dmg() {
    local dmg="$1"
    codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$dmg"
    codesign --verify --strict --verbose=2 "$dmg"
}

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    begin_step codesign "Signing gateway + engine + app + bundled Postgres tree with Developer ID."
    step "Codesigning bundled gateway + engine + app"
    sign_app_bundle "$APP_PATH"
    end_step codesign "Codesigned the bundle (gateway + engine + app + ~200 loose Postgres Mach-O files) with Developer ID."
fi

# ── 5c. refresh DMG payload + hide the stray .VolumeIcon.icns ───────────────
# create-dmg (what `cargo tauri build` shells out to) drops `.VolumeIcon.icns`
# at the volume root but, unlike `.DS_Store`, never sets its UF_HIDDEN flag — so
# on macOS setups where a leading dot alone isn't enough, it shows as a visible
# file in the install window next to the app. Mount the DMG read-write, set
# UF_HIDDEN on it, and recompress in place. The volume's custom icon still works
# (hiding the file doesn't disable it). Also replaces the app inside the DMG with
# the post-build app path, so manual resource signing above is actually reflected
# in the installer payload.
step "Refreshing DMG payload and hiding .VolumeIcon.icns"
refresh_dmg_payload() {
    local dmg="$1"
    local app="$2"
    local rw="${dmg%.dmg}.rw.dmg"
    local mnt
    mnt="$(mktemp -d)"
    rm -f "$rw"
    hdiutil convert "$dmg" -format UDRW -o "$rw" >/dev/null
    hdiutil attach "$rw" -nobrowse -noautoopen -mountpoint "$mnt" >/dev/null
    rm -rf "$mnt/$(basename "$app")"
    ditto "$app" "$mnt/$(basename "$app")"
    [ -f "$mnt/.VolumeIcon.icns" ] && chflags hidden "$mnt/.VolumeIcon.icns"
    hdiutil detach "$mnt" -force >/dev/null
    rmdir "$mnt" 2>/dev/null || true
    # Recompress to a temp path, then atomically swap onto the original — never
    # delete the only good artifact before its replacement is fully written, so
    # a failed recompress can't lose the (expensive) build output.
    local out="${dmg%.dmg}.zlib.dmg"
    rm -f "$out"
    hdiutil convert "$rw" -format UDZO -imagekey zlib-level=9 -o "$out" >/dev/null
    mv -f "$out" "$dmg"
    rm -f "$rw"
}
begin_step notarize "Refreshing DMG payload, signing the DMG, submitting to Apple notary service (--wait), stapling the ticket."
refresh_dmg_payload "$DMG_PATH" "$APP_PATH"

# ── 6. sign DMG + notarize (env-gated) ──────────────────────────────────────
# In --release mode signing + notarization are MANDATORY: a missing credential
# fails loud here rather than silently producing an un-notarized DMG (the v0.10.1
# "notarization silently skipped" fragility). In local mode they stay optional.
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    step "Codesigning DMG + notarizing"
    sign_dmg "$DMG_PATH"
    if [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
        xcrun notarytool submit "$DMG_PATH" --apple-id "$APPLE_ID" \
            --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" --wait
        xcrun stapler staple "$DMG_PATH"
        xcrun stapler staple "$APP_PATH"
    elif [ "$RELEASE_MODE" = "1" ]; then
        die "release mode requires notarization but APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID are not all set — refusing to ship an un-notarized DMG."
    else
        echo "    APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID not all set — skipping notarization."
    fi
elif [ "$RELEASE_MODE" = "1" ]; then
    die "release mode requires signing but APPLE_SIGNING_IDENTITY is not set."
else
    echo ""
    echo "NOTE: APPLE_SIGNING_IDENTITY not set — produced an UNSIGNED .dmg."
    echo "      Gatekeeper will block it on other Macs (right-click → Open to bypass locally)."
    echo "      Set the APPLE_* env vars to sign + notarize. See docs/desktop-app.md."
fi
end_step notarize "Notarized + stapled the DMG and the .app."

# ── 6b. upload assets to the GitHub Release (release mode) ───────────────────
# The deterministic build owns the asset upload: signed DMG + Tauri updater
# tarball + .sig + a generated latest.json, attached to the release tag. This is
# the cockpit `upload` step. release-to-lucidos.sh creates the Release first; we
# only attach assets to it.
upload_release_assets() {
    [ -n "$UPLOAD_TAG" ] || die "release upload requires --upload-tag"
    command -v gh >/dev/null 2>&1 || die "gh CLI required to upload release artifacts (https://cli.github.com/)."

    local app_tarball app_sig
    app_tarball="$(/usr/bin/find "$BUNDLE_DIR/macos" -name '*.app.tar.gz' 2>/dev/null | head -1 || true)"
    app_sig="$(/usr/bin/find "$BUNDLE_DIR/macos" -name '*.app.tar.gz.sig' 2>/dev/null | head -1 || true)"
    [ -n "$app_tarball" ] || die "no .app.tar.gz produced — is TAURI_SIGNING_PRIVATE_KEY set?"
    [ -n "$app_sig" ]     || die "no .app.tar.gz.sig produced — is TAURI_SIGNING_PRIVATE_KEY set?"

    # latest.json (the in-app auto-update manifest). The uploaded asset's name is
    # the file's basename, and the updater endpoint resolves
    # …/releases/latest/download/latest.json — so the file must literally be named
    # latest.json. Stage it under that exact name.
    local platform_key tarball_name download_url pub_date latest_dir latest_json
    case "$(uname -m)" in
        arm64|aarch64) platform_key="darwin-aarch64" ;;
        x86_64)        platform_key="darwin-x86_64" ;;
        *) die "unsupported arch for latest.json: $(uname -m)" ;;
    esac
    tarball_name="$(basename "$app_tarball")"
    download_url="https://github.com/$REPO_SLUG/releases/download/$UPLOAD_TAG/$tarball_name"
    pub_date="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    latest_dir="$(mktemp -d -t lucidos-latest)"
    latest_json="$latest_dir/latest.json"

    # python3 is present on any Mac with the Xcode CLT this script already needs.
    # It JSON-encodes the multi-line changelog notes + the signature safely. The
    # notes file is optional (empty notes if --notes-file was not supplied).
    RELEASE_VERSION="$EFFECTIVE_VERSION" PLATFORM_KEY="$platform_key" DOWNLOAD_URL="$download_url" \
    PUB_DATE="$pub_date" NOTES_FILE="${NOTES_FILE:-}" SIG_FILE="$app_sig" \
    python3 - > "$latest_json" <<'PY'
import json, os
notes_file = os.environ.get("NOTES_FILE", "")
notes = open(notes_file, encoding="utf-8").read().strip() if notes_file else ""
sig = open(os.environ["SIG_FILE"], encoding="utf-8").read().strip()
manifest = {
    "version": os.environ["RELEASE_VERSION"],
    "notes": notes,
    "pub_date": os.environ["PUB_DATE"],
    "platforms": {
        os.environ["PLATFORM_KEY"]: {
            "signature": sig,
            "url": os.environ["DOWNLOAD_URL"],
        }
    },
}
print(json.dumps(manifest, indent=2))
PY
    [ -s "$latest_json" ] || die "latest.json generation produced no output"

    # --clobber so a re-run replaces assets instead of erroring on existing names.
    gh release upload "$UPLOAD_TAG" --repo "$REPO_SLUG" --clobber \
        "$DMG_PATH" "$app_tarball" "$app_sig" "$latest_json" \
        || die "gh release upload failed for $UPLOAD_TAG"
    rm -rf "$latest_dir"
}

if [ "$RELEASE_MODE" = "1" ]; then
    begin_step upload "Generating latest.json and attaching the signed DMG + updater artifacts to $UPLOAD_TAG."
    step "Uploading DMG + updater artifacts to GitHub Release $UPLOAD_TAG"
    upload_release_assets
    end_step upload "Uploaded the signed DMG + updater tarball + .sig + latest.json to $UPLOAD_TAG."
fi

# ── 7. report ───────────────────────────────────────────────────────────────
step "Done"
echo "  .dmg:  $DMG_PATH"
[ -n "$APP_PATH" ] && echo "  .app:  $APP_PATH"
UPDATER_SIG="$(/usr/bin/find "$BUNDLE_DIR/macos" -name '*.app.tar.gz.sig' 2>/dev/null | head -1 || true)"
if [ -n "$UPDATER_SIG" ]; then
    echo "  updater artifacts: $(dirname "$UPDATER_SIG")/*.app.tar.gz(.sig)"
    echo "  → upload the .dmg, .app.tar.gz, .sig and a latest.json to a GitHub Release"
    echo "    (plugins.updater.endpoints in tauri.conf.json points at latest.json)."
else
    echo "  (no updater .sig — set TAURI_SIGNING_PRIVATE_KEY to emit signed update artifacts.)"
fi
