#!/usr/bin/env bash
# stage_runtime.sh — assemble the self-contained Lucidos runtime tree (the 7
# RESOURCE_NAMES: lucidos-engine + lucidos-gateway + lucidos (CLI) + frontend +
# relocatable PostgreSQL 18 + pgvector + sdk + system-knowhow) that both delivery
# vehicles share:
#
#   • build-dmg.sh        (macOS) stages into bundle-resources/, wraps it in a
#                         .app via `cargo tauri build`, codesigns, notarizes.
#   • build-headless.sh   (Linux + macOS) stages into a plain tree and tars it
#                         (step 2 of docs/plans/2026-06-30-installer-step2-linux-tarball.md).
#
# Everything here is the platform-agnostic spine of staging; the ONLY divergence
# between the two vehicles is what wraps the staged tree (Tauri/.app/sign vs. a
# plain tar) and the host-keyed pgvector PG_SYSROOT override (macOS only).
#
# These functions take everything explicitly (no build-dmg.sh / build-headless.sh
# globals) so scripts/lib/stage_runtime_test.sh exercises the pure ones — triple
# resolution, the theseus/pgvector URL builders, the Darwin-sysroot decision, and
# stage_runtime_assemble — offline with fake files. The build steps
# (build_frontend / build_binaries / fetch_postgres) need cargo/npm/network and are
# covered by a real build (the macOS build-headless smoke + the CI matrix).
#
# stage_runtime_fetch_postgres depends on nothing external; the assemble/build
# helpers shell out to cp/cargo/npm. No dependence on `set -e` in the caller —
# every step guards its own failure with `|| return 1`.

# ── target-triple resolution ─────────────────────────────────────────────────
# The theseus-rs relocatable-PostgreSQL release assets are named by the Rust
# target triple, e.g. postgresql-18.4.0-x86_64-unknown-linux-gnu.tar.gz. We resolve
# the SAME triple for the PG fetch and for the tarball name, from `uname`.

# stage_runtime_theseus_os <uname-s> — map `uname -s` to the triple's OS segment.
stage_runtime_theseus_os() {
    case "$1" in
        Darwin) printf 'apple-darwin' ;;
        Linux)  printf 'unknown-linux-gnu' ;;
        *) echo "ERROR: unsupported OS '$1' (expected Darwin or Linux)" >&2; return 1 ;;
    esac
}

# stage_runtime_arch <uname-m> — map `uname -m` to the triple's arch segment.
stage_runtime_arch() {
    case "$1" in
        arm64|aarch64) printf 'aarch64' ;;
        x86_64|amd64)  printf 'x86_64' ;;
        *) echo "ERROR: unsupported arch '$1' (expected arm64/aarch64 or x86_64)" >&2; return 1 ;;
    esac
}

# stage_runtime_triple <uname-s> <uname-m> — print the full target triple, e.g.
# x86_64-unknown-linux-gnu or aarch64-apple-darwin.
stage_runtime_triple() {
    local os arch
    arch="$(stage_runtime_arch "$2")" || return 1
    os="$(stage_runtime_theseus_os "$1")" || return 1
    printf '%s-%s' "$arch" "$os"
}

# stage_runtime_host_triple — the target triple of the current host.
stage_runtime_host_triple() {
    stage_runtime_triple "$(uname -s)" "$(uname -m)"
}

# stage_runtime_version <repo-root> <app-dir> — resolve the version stamped into
# artifact names: the RELEASE file at the repo root when present + non-empty, else
# the committed tauri.conf.json version, else 0.0.0. Mirrors how the .dmg is named.
stage_runtime_version() {
    local repo_root="$1" app_dir="$2" v=""
    [ -f "$repo_root/RELEASE" ] && v="$(tr -d '[:space:]' < "$repo_root/RELEASE")"
    if [ -n "$v" ]; then
        printf '%s' "$v"
        return 0
    fi
    python3 - "$app_dir/tauri.conf.json" <<'PY' 2>/dev/null || printf '0.0.0'
import json, sys
try:
    print(json.load(open(sys.argv[1]))["version"])
except Exception:
    print("0.0.0")
PY
}

# ── download URL builders (pure — unit-tested offline) ───────────────────────

# stage_runtime_pg_url <pg-version> <triple> — the theseus-rs relocatable PG asset.
stage_runtime_pg_url() {
    printf 'https://github.com/theseus-rs/postgresql-binaries/releases/download/%s/postgresql-%s-%s.tar.gz' \
        "$1" "$1" "$2"
}

# stage_runtime_pgvector_url <pgvector-version> — the pgvector source tarball.
stage_runtime_pgvector_url() {
    printf 'https://github.com/pgvector/pgvector/archive/refs/tags/v%s.tar.gz' "$1"
}

# stage_runtime_needs_macos_sysroot <uname-s> — return 0 on Darwin, 1 otherwise.
# The theseus tarball bakes its CI Xcode SDK path into PGXS, so compiling pgvector
# against it on a Mac needs PG_SYSROOT overridden to a local SDK. Linux uses system
# gcc and needs no override. We compile pgvector for the HOST (PGXS needs the host
# C toolchain), so this keys on the host OS, not the requested triple.
stage_runtime_needs_macos_sysroot() {
    [ "$1" = "Darwin" ]
}

# ── build steps (network / toolchain — not offline-testable) ─────────────────

# stage_runtime_build_frontend <repo-root> <app-dir> — build the bundled frontend
# (Vite dist/) and the JS SDK bundle. Mirrors build-dmg.sh's step 1.
stage_runtime_build_frontend() {
    local repo_root="$1" app_dir="$2"
    ( cd "$repo_root" && npm ci ) || return 1   # strict: install exactly from the committed package-lock.json
    ( cd "$repo_root/packages/lucidos-sdk" && npm run build ) || return 1   # /api/v1/sdk.js for app UIs
    ( cd "$app_dir" && npm run build ) || return 1
    [ -f "$app_dir/dist/index.html" ] || { echo "ERROR: frontend build did not produce $app_dir/dist/index.html" >&2; return 1; }
}

# stage_runtime_build_binaries <repo-root> <pkg…> — release-build the given cargo
# packages. The caller passes the package set it needs (both build-dmg.sh and
# build-headless.sh build engine + gateway + the load-bearing lucidos CLI).
stage_runtime_build_binaries() {
    local repo_root="$1"; shift
    [ "$#" -ge 1 ] || { echo "ERROR: stage_runtime_build_binaries needs at least one package" >&2; return 1; }
    local -a pkg_args=()
    local p
    for p in "$@"; do pkg_args+=(-p "$p"); done
    # .cargo/config.toml sets rustc-wrapper=sccache; disable it if sccache is absent
    # so the build doesn't fail on a missing wrapper (an empty RUSTC_WRAPPER env var
    # overrides the config). CI runners have no sccache; this is the same guard
    # build-dmg.sh has always used.
    command -v sccache >/dev/null || export RUSTC_WRAPPER=""
    ( cd "$repo_root" && cargo build --locked "${pkg_args[@]}" --release ) || return 1
}

# stage_runtime_fetch_postgres <pg-version> <pgvector-version> <triple> <work-dir>
# Fetch the relocatable PostgreSQL <pg-version> for <triple> into <work-dir> and
# compile pgvector <pgvector-version> against it (cached: re-extract / re-build only
# when missing). Prints the PG prefix dir on stdout; ALL progress goes to stderr so
# the caller can capture the prefix via command substitution. Mirrors build-dmg.sh's
# step 3 / scripts/prototype/desktop-pg-pgvector-spike.sh — the proven recipe.
stage_runtime_fetch_postgres() {
    local pg_version="$1" pgvector_version="$2" triple="$3" work="$4"
    mkdir -p "$work" || return 1

    local dirname="postgresql-${pg_version}-${triple}"
    local prefix="$work/$dirname"
    local pgconfig="$prefix/bin/pg_config"

    if [ ! -x "$pgconfig" ]; then
        echo "--- fetching relocatable PostgreSQL $pg_version ($triple)" >&2
        curl -fsSL -m 300 -o "$work/$dirname.tar.gz" "$(stage_runtime_pg_url "$pg_version" "$triple")" >&2 || return 1
        tar -xzf "$work/$dirname.tar.gz" -C "$work" >&2 || return 1
    fi
    [ -x "$pgconfig" ] || { echo "ERROR: pg_config missing after extract ($pgconfig)" >&2; return 1; }

    local sharedir
    sharedir="$("$pgconfig" --sharedir)" || return 1
    if [ ! -f "$sharedir/extension/vector.control" ]; then
        echo "--- compiling pgvector $pgvector_version against the bundled PG" >&2
        curl -fsSL -m 180 -o "$work/pgvector.tar.gz" "$(stage_runtime_pgvector_url "$pgvector_version")" >&2 || return 1
        rm -rf "$work/pgvector" && mkdir -p "$work/pgvector" || return 1
        tar -xzf "$work/pgvector.tar.gz" -C "$work/pgvector" --strip-components=1 >&2 || return 1

        local -a make_args=(PG_CONFIG="$pgconfig")
        if stage_runtime_needs_macos_sysroot "$(uname -s)"; then
            # Override the theseus tarball's baked-in CI SDK path with a local SDK.
            make_args+=(PG_SYSROOT="$(xcrun --show-sdk-path)")
        fi
        ( cd "$work/pgvector" \
            && make -s "${make_args[@]}" \
            && make -s install "${make_args[@]}" ) >&2 || return 1
    fi
    [ -f "$sharedir/extension/vector.control" ] || { echo "ERROR: pgvector did not install into the bundled PG" >&2; return 1; }

    printf '%s\n' "$prefix"
}

# ── assemble (offline — unit-tested with fake inputs) ────────────────────────

# stage_runtime_assemble <stage-dir> <engine-bin> <gateway-bin> <cli-bin> \
#                        <frontend-dist> <pg-prefix> <sdk-dist> <system-knowhow-dir>
# Assemble the 7-resource self-contained runtime tree into a CLEAN <stage-dir>:
#   lucidos-engine, lucidos-gateway, lucidos, frontend/, postgres/, sdk/,
#   system-knowhow/.
# Pure cp/chmod over already-built inputs (no network/toolchain), so the unit test
# drives it with fakes. Prints <stage-dir> on success. Mirrors build-dmg.sh's step 4.
#
# `lucidos` (the CLI) is load-bearing, not optional: the engine resolves it as a
# sibling of `lucidos-engine` (find_lucidos_cli_dir) to launch the Claude Code
# permission-prompt MCP server (`lucidos mcp-permission-server`). Omitting it
# breaks every coding-agent thread in the packaged build on its first tool call.
#
# `system-knowhow/` is the engine-shipped reference set the workspace LLM is told
# to load (load_knowhow('system-knowhow/…'), GET /api/v1/knowhow, the data-API
# read path). The engine finds it via LUCIDOS_SYSTEM_KNOWHOW_DIR (set by the
# desktop/gateway launcher + install service to <resources>/system-knowhow);
# omitting it silently degrades every packaged install to no reference docs.
stage_runtime_assemble() {
    local stage="$1" engine="$2" gateway="$3" cli="$4" frontend="$5" pg_prefix="$6" sdk="$7" system_knowhow="$8"
    [ -x "$engine" ]    || { echo "ERROR: engine binary not found/executable: $engine" >&2; return 1; }
    [ -x "$gateway" ]   || { echo "ERROR: gateway binary not found/executable: $gateway" >&2; return 1; }
    [ -x "$cli" ]       || { echo "ERROR: lucidos CLI binary not found/executable: $cli" >&2; return 1; }
    [ -d "$frontend" ]  || { echo "ERROR: frontend dist not found: $frontend" >&2; return 1; }
    [ -d "$pg_prefix" ] || { echo "ERROR: postgres prefix not found: $pg_prefix" >&2; return 1; }
    [ -d "$sdk" ]       || { echo "ERROR: sdk dist not found: $sdk" >&2; return 1; }
    [ -d "$system_knowhow" ] || { echo "ERROR: system-knowhow dir not found: $system_knowhow" >&2; return 1; }

    rm -rf "$stage" || return 1
    mkdir -p "$stage" || return 1
    cp "$engine" "$stage/lucidos-engine"   || return 1
    cp "$gateway" "$stage/lucidos-gateway" || return 1
    # The `lucidos` CLI — sibling of the engine so find_lucidos_cli_dir resolves
    # it for the CC permission MCP server, the lucidos-cli skill, and script PATH.
    cp "$cli" "$stage/lucidos"             || return 1
    chmod +x "$stage/lucidos-engine" "$stage/lucidos-gateway" "$stage/lucidos" || return 1
    cp -R "$frontend" "$stage/frontend"    || return 1
    cp -R "$pg_prefix" "$stage/postgres"   || return 1
    # The JS SDK (/api/v1/sdk.js) used by app-UI iframes; the engine finds it via
    # LUCIDOS_SDK_DIR (set by the desktop/gateway launcher to <resources>/sdk).
    cp -R "$sdk" "$stage/sdk"              || return 1
    # The engine-shipped reference knowhow, resolved at runtime via
    # LUCIDOS_SYSTEM_KNOWHOW_DIR (→ <resources>/system-knowhow).
    cp -R "$system_knowhow" "$stage/system-knowhow" || return 1

    printf '%s\n' "$stage"
}

# ── staged-knowhow freshness (the hand-run-build guard) ──────────────────────

# stage_runtime_staged_knowhow_fresh <stage-dir> <system-knowhow-dir>
# Exit 0 when <stage-dir>/system-knowhow either does not exist or is byte-identical
# to <system-knowhow-dir>; non-zero with the diff on stderr when it has drifted.
#
# WHY THIS EXISTS. stage_runtime_assemble above rm -rf's the stage and re-copies,
# so every sanctioned build path is correct by construction. What is NOT correct
# by construction is the stage BETWEEN builds: build-dmg.sh's stage is
# crates/lucidos-app/bundle-resources/, it is gitignored, and it survives from one
# run to the next. A `cargo tauri build --config '<resource map>'` typed by hand
# skips step 4 and hands Tauri whatever that leftover holds, so a months-old copy
# of system-knowhow/ gets packaged and nothing says a word. That is the hole: not
# a wrong copy (the builds fix that), a stale one nobody is told about. Found
# 2026-08-07 with 12,732 chars of stale descriptions staged against 6,584 live,
# including a whole doc that had been rewritten.
#
# system-knowhow/ is the only staged resource this can be asked of: it is the one
# that is a git-tracked SOURCE tree with an exact live counterpart, so the
# comparison is a byte diff. The binaries, postgres/, frontend/ and sdk/ are build
# outputs where "fresh" is undefined without rebuilding them, and a timestamp
# heuristic there would fail open.
#
# An absent stage, or a stage with no system-knowhow/ in it, is CLEAN. A developer
# who has never run build-dmg.sh has no bundle-resources/ at all, and this must not
# turn their `cargo tauri build` red. The fast path tests -e rather than -d so that
# ABSENT is the only thing it waves through: a staged system-knowhow that is not a
# directory is something gone wrong, and `diff -r` reports the type mismatch as the
# drift it is instead of the guard reading it as nothing to check.
stage_runtime_staged_knowhow_fresh() {
    local stage="$1" system_knowhow="$2" staged drift
    staged="$stage/system-knowhow"
    [ -e "$staged" ] || return 0
    [ -d "$system_knowhow" ] || {
        echo "ERROR: system-knowhow dir not found: $system_knowhow" >&2
        return 1
    }

    drift="$(diff -r "$system_knowhow" "$staged" 2>&1)" && return 0

    {
        echo "ERROR: the staged system-knowhow copy has drifted from the live tree."
        echo "  live:   $system_knowhow"
        echo "  staged: $staged"
        echo ""
        echo "$drift"
        echo ""
        echo "A packaged build would ship the STAGED text, so the workspace LLM would"
        echo "route on descriptions that no longer match the docs. Restage it:"
        echo "  ./scripts/build-dmg.sh          # restages from scratch, then builds"
        echo "or drop the leftover so the next build rebuilds it:"
        echo "  rm -rf '$stage'"
    } >&2
    return 1
}
