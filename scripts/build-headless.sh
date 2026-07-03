#!/usr/bin/env bash
#
# build-headless.sh — build the self-contained, Tauri-free Lucidos runtime tarball:
# the headless twin of build-dmg.sh's .app/.dmg. This is the Linux build path
# (step 2 of docs/plans/2026-06-30-installer-step2-linux-tarball.md) and also
# runs on macOS as the local smoke for the shared staging recipe.
#
# It assembles the SAME 6-resource self-contained tree as the DMG —
#   • lucidos-engine        (target/release/lucidos-engine)
#   • lucidos-gateway       (target/release/lucidos-gateway)
#   • lucidos               (target/release/lucidos — the CLI)
#   • frontend              (crates/lucidos-app/dist)
#   • postgres              (relocatable PostgreSQL 18 + compiled pgvector)
#   • sdk                   (packages/lucidos-sdk/dist)
# — via the shared scripts/lib/stage_runtime.sh, then reuses
# scripts/lib/headless_tarball.sh to emit lucidos-<version>-<triple>.tar.gz plus a
# `shasum -a 256 -c`-compatible .sha256 sidecar. No Tauri, no .app, no DMG.
#
# Platform notes:
#   • Linux: this is THE release build path. The CI matrix
#     (.github/workflows/release-tarballs.yml) runs it per target triple on a
#     native runner. The theseus PG18 asset + pgvector compile resolve the Linux
#     relocatable Postgres automatically from the host triple. No codesigning.
#   • macOS: this produces an UNSIGNED tarball — useful to verify the shared
#     staging path end-to-end locally. The SIGNED macOS tarball ships from
#     `scripts/build-dmg.sh --emit-tarball`, which sources the Developer-ID-signed
#     .app Resources. Do not ship a build-headless macOS tarball.
#
# It compiles the engine/gateway + pgvector NATIVELY, so --triple must match the
# host (cross-arch artifacts come from the CI matrix's per-arch runners, not from
# cross-compiling here).
#
# Usage:
#   ./scripts/build-headless.sh                 # build for the host triple
#   ./scripts/build-headless.sh --check         # validate the resource contract (offline)
#   ./scripts/build-headless.sh --triple x86_64-unknown-linux-gnu
#   ./scripts/build-headless.sh --out-dir dist/headless --version 0.14.0
#
# Flags:
#   --triple TRIPLE   target triple (default: host). Must equal the host triple.
#   --out-dir DIR     where to write the tarball + .sha256
#                     (default: .lucidos/release-staging/<version>/)
#   --version V       version stamped into the tarball name
#                     (default: RELEASE → tauri.conf.json → 0.0.0)
#   --check           print the resource contract for the resolved triple and exit
#   -h, --help        this help
#
# Env knobs (all optional, shared with build-dmg.sh):
#   PG_VERSION        relocatable PostgreSQL version   (default 18.4.0)
#   PGVECTOR_VERSION  pgvector tag to compile          (default 0.8.2)

set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_DIR="$REPO_ROOT/crates/lucidos-app"
RESOURCE_NAMES=(lucidos-engine lucidos-gateway lucidos frontend postgres sdk)

PG_VERSION="${PG_VERSION:-18.4.0}"          # match build-dmg.sh / the dev/docker stack
PGVECTOR_VERSION="${PGVECTOR_VERSION:-0.8.2}"

# sha256 helper (release_staging_sha256) — headless_tarball.sh depends on it, so
# source it first. Both are pure shell + python3, public-mirror-safe.
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/lib/release_staging.sh"
# shellcheck source=scripts/lib/headless_tarball.sh
source "$SCRIPT_DIR/lib/headless_tarball.sh"
# Shared staging recipe (also used by build-dmg.sh).
# shellcheck source=scripts/lib/stage_runtime.sh
source "$SCRIPT_DIR/lib/stage_runtime.sh"

step() { printf '\n==> %s\n' "$*"; }
die()  { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage:
  ./scripts/build-headless.sh                 # build for the host triple
  ./scripts/build-headless.sh --check         # validate the resource contract (offline)
  ./scripts/build-headless.sh --triple x86_64-unknown-linux-gnu
  ./scripts/build-headless.sh --out-dir dist/headless --version 0.14.0

Flags:
  --triple TRIPLE   target triple (default: host). Must equal the host triple.
  --out-dir DIR     where to write the tarball + .sha256
                    (default: .lucidos/release-staging/<version>/)
  --version V       version stamped into the tarball name
                    (default: RELEASE -> tauri.conf.json -> 0.0.0)
  --check           print the resource contract for the resolved triple and exit
  -h, --help        this help

Env knobs (all optional, shared with build-dmg.sh):
  PG_VERSION        relocatable PostgreSQL version   (default 18.4.0)
  PGVECTOR_VERSION  pgvector tag to compile          (default 0.8.2)

build-headless.sh emits the Tauri-free lucidos-<version>-<triple>.tar.gz (engine +
gateway + frontend + relocatable PostgreSQL 18 + pgvector + sdk) + a .sha256 sidecar.
On Linux it is the release build path (see .github/workflows/release-tarballs.yml);
on macOS it produces an UNSIGNED tarball (ship the signed one via
scripts/build-dmg.sh --emit-tarball).
EOF
}

TRIPLE=""
OUT_DIR=""
VERSION_ARG=""
DO_CHECK=0
while [ $# -gt 0 ]; do
    case "$1" in
        --triple)   [ $# -ge 2 ] || die "--triple requires an argument";  TRIPLE="$2";      shift 2 ;;
        --out-dir)  [ $# -ge 2 ] || die "--out-dir requires an argument"; OUT_DIR="$2";     shift 2 ;;
        --version)  [ $# -ge 2 ] || die "--version requires an argument"; VERSION_ARG="$2"; shift 2 ;;
        --check)    DO_CHECK=1; shift ;;
        -h|--help)  usage; exit 0 ;;
        *)          die "unknown argument: $1" ;;
    esac
done

# Resolve the target triple (host, or --triple).
if [ -z "$TRIPLE" ]; then
    TRIPLE="$(stage_runtime_host_triple)" \
        || die "could not resolve the target triple for $(uname -s)/$(uname -m)"
fi

# --check: validate the resource contract offline and exit (no build, no network).
if [ "$DO_CHECK" = "1" ]; then
    printf 'OK: headless bundle for %s includes %s\n' "$TRIPLE" "${RESOURCE_NAMES[*]}"
    exit 0
fi

# We compile the engine/gateway + pgvector natively, so the requested triple must
# match the host. Cross-arch artifacts come from the CI matrix's per-arch runners.
HOST_TRIPLE="$(stage_runtime_host_triple)" \
    || die "could not resolve the host triple for $(uname -s)/$(uname -m)"
if [ "$TRIPLE" != "$HOST_TRIPLE" ]; then
    die "requested --triple '$TRIPLE' != host '$HOST_TRIPLE'. build-headless.sh compiles natively; run it on a '$TRIPLE' runner. For all platforms at once use .github/workflows/release-tarballs.yml."
fi

VERSION="${VERSION_ARG:-$(stage_runtime_version "$REPO_ROOT" "$APP_DIR")}"
OUT_DIR="${OUT_DIR:-$REPO_ROOT/.lucidos/release-staging/$VERSION}"

command -v cargo >/dev/null || die "cargo not found — install Rust (https://rustup.rs)."
command -v npm   >/dev/null || die "npm not found — install Node.js."
command -v curl  >/dev/null || die "curl not found."
command -v tar   >/dev/null || die "tar not found."

step "Building headless bundle lucidos-$VERSION-$TRIPLE"

# ── 1. frontend ──────────────────────────────────────────────────────────────
step "Building frontend (dist/) + SDK bundle"
stage_runtime_build_frontend "$REPO_ROOT" "$APP_DIR" || die "frontend build failed"

# ── 2. engine + gateway + cli (release) ──────────────────────────────────────
# All three are part of the runtime tree. The `lucidos` CLI is load-bearing: the
# engine resolves it as a sibling of `lucidos-engine` to launch the CC
# permission-prompt MCP server, so a headless install that omits it breaks every
# coding-agent thread on its first tool call.
step "Building engine + gateway + cli (release)"
stage_runtime_build_binaries "$REPO_ROOT" lucidos-engine lucidos-gateway lucidos-cli \
    || die "engine + gateway + cli build failed"
ENGINE_BIN="$REPO_ROOT/target/release/lucidos-engine"
GATEWAY_BIN="$REPO_ROOT/target/release/lucidos-gateway"
CLI_BIN="$REPO_ROOT/target/release/lucidos"
[ -x "$ENGINE_BIN" ]  || die "engine binary not found at $ENGINE_BIN"
[ -x "$GATEWAY_BIN" ] || die "gateway binary not found at $GATEWAY_BIN"
[ -x "$CLI_BIN" ]     || die "lucidos CLI binary not found at $CLI_BIN"

# ── 3. relocatable PostgreSQL + pgvector ─────────────────────────────────────
step "Fetching relocatable PostgreSQL $PG_VERSION + building pgvector $PGVECTOR_VERSION ($TRIPLE)"
PG_PREFIX="$(stage_runtime_fetch_postgres "$PG_VERSION" "$PGVECTOR_VERSION" "$TRIPLE" "$REPO_ROOT/.lucidos/headless-build/pg")" \
    || die "failed to fetch/compile relocatable PostgreSQL + pgvector for $TRIPLE"

# ── 4. assemble the runtime tree ─────────────────────────────────────────────
STAGE="$REPO_ROOT/.lucidos/headless-build/$TRIPLE/stage"
step "Staging runtime tree → $STAGE"
stage_runtime_assemble "$STAGE" "$ENGINE_BIN" "$GATEWAY_BIN" "$CLI_BIN" "$APP_DIR/dist" \
    "$PG_PREFIX" "$REPO_ROOT/packages/lucidos-sdk/dist" >/dev/null \
    || die "failed to stage the runtime tree into $STAGE"

# ── 5. emit the headless tarball + .sha256 ───────────────────────────────────
step "Emitting headless tarball + .sha256 → $OUT_DIR"
TARBALL="$(headless_tarball_emit "$STAGE" "$OUT_DIR" "$VERSION" "$TRIPLE" "${RESOURCE_NAMES[@]}")" \
    || die "failed to emit the headless tarball"

step "Done"
echo "  tarball: $TARBALL"
echo "  sha256:  $TARBALL.sha256"
if [ "$(uname -s)" = "Darwin" ]; then
    echo ""
    echo "  NOTE: this macOS tarball is UNSIGNED (built without Developer ID)."
    echo "        Ship the SIGNED macOS tarball from: scripts/build-dmg.sh --emit-tarball"
fi
