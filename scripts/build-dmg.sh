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
# Build prereqs: the engine statically vendors OpenSSL (openssl-sys/vendored via
# crates/lucidos-engine/Cargo.toml), so the engine compile needs `perl` + a C
# toolchain — both already provided by the Xcode Command Line Tools this script
# relies on (clang, xcrun). This vendoring is unconditional in the manifest, so the
# dev build (`web-dev.sh -b`) and this packaged build link the identical OpenSSL.
#
# ── Script-vs-LLM contract (release pipeline) ────────────────────────────────
# The deterministic shell pipeline is the SPINE; the LLM/chat layer only drafts
# the changelog, gets approval (the `draft` step), and handles anomalies.
#
# In release mode this script OWNS the build → codesign → notarize → staple →
# upload-asset stages, and emits its own ReleaseStep* domain events at each stage
# boundary (via scripts/lib/release_events.sh) so the Release Cockpit app lights
# up stage by stage. The cockpit is a PURE READ-ONLY CONSUMER — this script never
# writes to it. Each stage emits ReleaseStepFailed (not a silent exit) on error so
# the cockpit shows red. release.sh / release-to-lucidos.sh own the surrounding
# git / tag / GitHub-Release / changelog stages and the final LucidosReleased.
#
# ── Build-once / verify-first / publish-verified (split build from upload) ────
# The release is split so the DMG you manually verify is bit-for-bit the DMG that
# ships. Three release modes:
#   --release-build   build → codesign → notarize → staple, then STAGE the .dmg +
#                     updater .app.tar.gz + .sig into
#                     .lucidos/release-staging/<version>/ with a manifest.json
#                     ({version, source_commit, artifacts:[{name,sha256}]}). Emits
#                     the build/codesign/notarize steps. NO upload, NO Release.
#   --release-attach  --staging-dir <d> --upload-tag <t> --notes-file <f>:
#                     VERIFY that staging (refuse on missing manifest / missing
#                     artifact / checksum mismatch), generate latest.json from the
#                     STAGED .sig + notes, and upload the staged artifacts. Emits
#                     the upload step. NO rebuild, NO signing creds needed.
#   --release         back-compat one-shot = --release-build immediately followed
#                     by --release-attach against the freshly-staged dir.
#
# Credentials in --release mode come ONLY from the auto-injected environment
# (APPLE_ID / APPLE_TEAM_ID are DB env vars; APPLE_PASSWORD /
# TAURI_SIGNING_PRIVATE_KEY_PATH are credentials mapped to those names). This
# script never re-exports or overrides the Apple secrets — it asserts each is
# non-empty at startup and fails loud if one is missing (the v0.10.1 clobber bug
# was an empty `export APPLE_ID="$CRED_APPLE_ID"` in the hand-improvised LLM layer).
#
# The updater key is the one exception that IS resolved: supply its FILE PATH via
# TAURI_SIGNING_PRIVATE_KEY_PATH (e.g. ~/.tauri/lucidos-updater.key) and the script
# loads the contents into TAURI_SIGNING_PRIVATE_KEY — the only name Tauri's bundler
# reads. For back-compat, TAURI_SIGNING_PRIVATE_KEY set directly (contents, or a
# path Tauri auto-detects) is still honored and left untouched.
#
# ── Resumable notarization (submit ≠ wait) ───────────────────────────────────
# Apple's notary service regularly takes longer than the process waiting on it
# lives. So the notarize stage NEVER holds a foreground `--wait`: it submits with
# `--no-wait`, PERSISTS the submission id to a resume handle
# (.lucidos/release-state/notarize-<version>.json — see scripts/lib/release_notarize.sh)
# BEFORE any waiting, and only then polls `notarytool info` for the verdict.
#
# Losing the waiter therefore costs a poll, not a rebuild. Any build-grade run
# that finds a resumable handle for its version (state present, DMG on disk still
# hashing to what was submitted, source_commit still HEAD) skips build + codesign
# + submit and goes straight to poll → staple → stage:
#   --resume-notarize          resume deliberately (fails loud if not resumable)
#   --adopt-submission <uuid>  write a handle for an in-flight submission whose id
#                              was never persisted (the DMG on disk is the one
#                              Apple is scanning), then resume
# The handle is dropped once staging succeeds, so a later run can't resume a
# finished release. This exists because the orchestration layer caps background
# tasks at 3600s: a notarization slower than that can never be held in a
# foreground wait, so resumability is the only fix.
#
# ── Deferred DMG (--defer-notarization) ──────────────────────────────────────
# Resumability keeps a slow verdict from costing a rebuild, but the RELEASE still
# waited on it — for 1 to 20 hours, every time. It never had to: notarization
# gates exactly one artifact, the DMG a browser downloads. The headless tarball
# (`curl | sh`) and the Tauri updater (.app.tar.gz + .sig + latest.json) are never
# quarantined, so Gatekeeper never assesses them; the updater's integrity comes
# from our own minisign key and the bundle launches on its Developer ID signature.
#
# --defer-notarization therefore submits, persists the handle, and STAGES THE
# UNSTAPLED DMG (manifest `notarized: false`) so the release publishes now. The
# submission stays in flight, so this path deliberately does NOT drop the resume
# handle or the submitted-bytes pin — they are the attach step's only inputs.
# Combined with --resume-notarize it stages an ALREADY-submitted build without
# polling at all, which is how a Phase A stuck on a slow verdict is rescued.
#
# What makes it safe rather than sloppy: the flag is explicit (no path falls back
# to it), the state travels in the manifest rather than in an operator's memory,
# and every public consumer derives its behaviour from that field — so the DMG
# cannot reach a Release without its "notarization pending" banner, and the site's
# Download-for-Mac link stays on the last notarized release until
# `release.sh --attach-notarized <version>` staples and swaps the asset.
#
# ── Headless tarball (--emit-tarball) ────────────────────────────────────────
# In ADDITION to the .app/.dmg, --emit-tarball packages the self-contained runtime
# tree (engine + gateway + frontend + postgres + sdk) as a plain, headless
# lucidos-<version>-<target-triple>.tar.gz plus a sha256 sidecar — the download
# artifact a later install.sh lays down instead of compiling from source (step 1 of
# docs/plans/2026-06-30-installer-step1-headless-tarball.md). It is sourced from the SIGNED
# .app Resources, so the Mach-O files inside the tarball keep their Developer ID
# signatures. The flag applies to any BUILD mode and changes nothing when absent;
# it is a no-op under --check and a build-less --release-attach (neither builds a
# .app to package).
#
# Usage:
#   ./scripts/build-dmg.sh                 # build an unsigned local .dmg (no events)
#   ./scripts/build-dmg.sh --check         # validate the staged resource contract
#   ./scripts/build-dmg.sh --emit-tarball  # also emit the headless .tar.gz + .sha256
#   APPLE_SIGNING_IDENTITY="Developer ID Application: …" \
#   APPLE_ID=… APPLE_PASSWORD=… APPLE_TEAM_ID=… \
#   TAURI_SIGNING_PRIVATE_KEY_PATH=~/.tauri/lucidos-updater.key \
#   TAURI_SIGNING_PRIVATE_KEY_PASSWORD=… \
#     ./scripts/build-dmg.sh               # signed + notarized (local, no events/upload)
#   ./scripts/build-dmg.sh --release-build # build + sign + notarize + STAGE (no upload)
#   ./scripts/build-dmg.sh --release-attach \
#     --staging-dir <dir> --upload-tag v<N.N.N> --notes-file <changelog-section>
#                                          # verify staging + upload it (no rebuild)
#   ./scripts/build-dmg.sh --release \
#     --release-version <N.N.N> --upload-tag v<N.N.N> \
#     --notes-file <changelog-section> --repo-slug <owner/repo>
#                                          # one-shot (build-then-attach), back-compat
#
# Release-mode flags:
#   --release            one-shot: --release-build then --release-attach (back-compat)
#   --release-build      build + sign + notarize + stage; no upload, no Release
#   --release-attach     verify --staging-dir + upload it; no rebuild, no signing
#   --staging-dir DIR    staging dir (required for --release-attach; default for the
#                        build modes is .lucidos/release-staging/<version>)
#   --release-version V  expected version; must equal the RELEASE file / manifest
#   --upload-tag TAG     GitHub Release tag to attach the DMG + updater assets to
#   --notes-file FILE    CHANGELOG section used as the latest.json `notes`
#   --repo-slug OWNER/R  GitHub repo for the release (default lucidos-dev/lucidos)
#
# Notarization-resume flags (see "Resumable notarization" above):
#   --resume-notarize        poll + staple + stage an already-submitted DMG; no build
#   --adopt-submission UUID  record an in-flight submission for the on-disk DMG,
#                            then resume (implies --resume-notarize)
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
# `lucidos` (the CLI) is a bundled Mach-O executable too — it MUST be codesigned
# (BUNDLED_EXECUTABLES) or notarization rejects it, and it MUST be staged
# (RESOURCE_NAMES) or the engine can't launch the CC permission MCP server.
BUNDLED_EXECUTABLES=(lucidos-engine lucidos-gateway lucidos)
RESOURCE_NAMES=(lucidos-engine lucidos-gateway lucidos frontend postgres sdk system-knowhow)

PG_VERSION="${PG_VERSION:-18.4.0}"   # match the dev/docker stack (pgvector/pgvector:pg18)
PGVECTOR_VERSION="${PGVECTOR_VERSION:-0.8.2}"

# Shared ReleaseStep* / LucidosReleased emit helpers (the cockpit contract).
# This lib is internal release tooling and is stripped from the public mirror, so
# source it only when present and fall back to no-op stubs otherwise -- the DMG
# build itself never depends on event emission.
# shellcheck source=scripts/lib/release_events.sh
if [ -f "$SCRIPT_DIR/lib/release_events.sh" ]; then
    source "$SCRIPT_DIR/lib/release_events.sh"
else
    emit_release_step() { :; }
    emit_lucidos_released() { :; }
    emit_release_dmg_notarized() { :; }
fi

# Tauri updater-key resolution (TAURI_SIGNING_PRIVATE_KEY_PATH → contents export).
# shellcheck source=scripts/lib/tauri_signing_key.sh
source "$SCRIPT_DIR/lib/tauri_signing_key.sh"

# Staging-manifest helpers for the build-once / verify-then-publish flow
# (--release-build stages + writes manifest.json; --release-attach verifies it +
# uploads). Pure shell + python3 and carries nothing sensitive, so — unlike
# release_events.sh above — it is NOT stripped from the public mirror; source it
# unconditionally, like tauri_signing_key.sh.
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/lib/release_staging.sh"

# Headless-tarball packaging (the --emit-tarball flag, step 1 of
# docs/plans/2026-06-30-installer-step1-headless-tarball.md). Pure tar/gzip + the sha256
# helper above; depends on release_staging_sha256, so source it AFTER
# release_staging.sh. Public-mirror-safe like release_staging.sh — source it
# unconditionally.
# shellcheck source=scripts/lib/headless_tarball.sh
source "$SCRIPT_DIR/lib/headless_tarball.sh"

# Notarize resume handle (the state file that makes the notary wait resumable —
# see the "Resumable notarization" block above). Also depends on
# release_staging_sha256, so it too is sourced AFTER release_staging.sh.
# Public-mirror-safe; source it unconditionally.
# shellcheck source=scripts/lib/release_notarize.sh
source "$SCRIPT_DIR/lib/release_notarize.sh"

# Compiled-input fingerprint (scripts/lib/release_build_fingerprint.sh): lets a
# release re-fold tell "the shipped bytes changed" from "a docs commit landed",
# so a rebuild + a fresh Apple notarization submission is spent only when it can
# actually change the artifact. Stamped into the staging manifest here; READ by
# release.sh's re-fold gate. Pure git plumbing, public-mirror-safe.
# shellcheck source=scripts/lib/release_build_fingerprint.sh
source "$SCRIPT_DIR/lib/release_build_fingerprint.sh"

# Shared staging library (step 2 of the installer rework): the platform-agnostic
# spine of staging — target-triple resolution, the theseus PG18 + pgvector
# fetch/compile recipe, the frontend/binary builds, and the 7-resource assemble.
# build-dmg.sh (macOS/.app) and scripts/build-headless.sh (Linux + macOS headless)
# both source it so the staging recipe lives in exactly one place. Pure helpers,
# public-mirror-safe — source it unconditionally like release_staging.sh.
# shellcheck source=scripts/lib/stage_runtime.sh
source "$SCRIPT_DIR/lib/stage_runtime.sh"

# Release-mode state (set by arg parsing below). In default (local-build) mode
# all stay 0: no events, no asserted creds, no staging, no asset upload.
#   RELEASE_MODE  1 for any --release* mode (drives event emission + assertions)
#   DO_BUILD      1 for --release-build / --release (build → sign → notarize →
#                 stage; signing creds asserted)
#   DO_ATTACH     1 for --release-attach / --release (upload staged artifacts)
RELEASE_MODE=0
DO_BUILD=0
DO_ATTACH=0
RELEASE_VERSION_ARG=""
UPLOAD_TAG=""
NOTES_FILE=""
REPO_SLUG="lucidos-dev/lucidos"
STAGING_DIR_ARG=""     # --staging-dir override (required for --release-attach)
STAGING_DIR=""         # resolved staging dir (default .lucidos/release-staging/<v>)
EFFECTIVE_VERSION=""   # the version stamped into the DMG; set after arg parse
CURRENT_STEP=""        # cockpit step id currently in flight (for failure emit)
EMIT_TARBALL=0         # 1 for --emit-tarball: also emit the headless .tar.gz + .sha256
HEADLESS_TARBALL_PATH="" # set by emit_headless_tarball for the final report

# Resumable-notarization state (see the header block). DO_RESUME_NOTARIZE is set
# by --resume-notarize / --adopt-submission, and also by the automatic detection
# of a resumable handle in a build-grade run.
DO_RESUME_NOTARIZE=0
ADOPT_SUBMISSION=""          # --adopt-submission <uuid>
NOTARIZE_STATE_FILE=""       # resolved once EFFECTIVE_VERSION is known
NOTARIZE_SUBMISSION_ID=""    # set by notarize_submit
NOTARIZE_STATUS=""           # set by notarize_poll (the terminal Apple verdict)
NOTARIZE_SUBMITTED_SHA=""    # sha256 of the DMG as submitted; asserted before stapling
NOTARIZE_PINNED_DMG=""       # immutable hardlink to the submitted bytes (see notarize_pin_submitted_dmg)

# Deferred-DMG release (--defer-notarization). Apple's verdict routinely takes
# 1–20 hours, and everything the release actually needs — the tarball, the
# updater artifacts, the tree itself — is ready long before it lands. Deferring
# submits, persists the resume handle, and STAGES THE UNSTAPLED DMG so the
# release can publish now; release.sh --attach-notarized staples and replaces the
# asset once the ticket arrives. See docs/plans/2026-07-29-deferred-dmg-release-mode.md.
#
# DMG_NOTARIZED_STATE is what lands in the staging manifest, and it is the single
# fact every public-facing consumer reads (banner, site link, cleanup). It stays
# "true" on every other path, so nothing but this flag can produce a pending
# staging.
DEFER_NOTARIZATION=0
DMG_NOTARIZED_STATE="true"
# Set by --allow-pending-notarization: the CALLER asserts it has published the
# "notarization pending" banner alongside this upload. Only release-to-lucidos.sh
# passes it, and only on the branch where it actually composed that banner — so
# the flag is the explicit hand-off of that promise, not a general override.
ALLOW_PENDING_NOTARIZATION=0
# Poll cadence + ceilings. The interval is short enough to finish promptly and
# long enough not to hammer Apple; the timeout only bounds THIS process — the
# resume handle outlives it, so hitting it costs another poll, never a rebuild.
NOTARIZE_POLL_INTERVAL="${NOTARIZE_POLL_INTERVAL:-30}"
NOTARIZE_POLL_TIMEOUT="${NOTARIZE_POLL_TIMEOUT:-7200}"
NOTARIZE_POLL_MAX_FAILURES="${NOTARIZE_POLL_MAX_FAILURES:-5}"

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
    # By here resolve_tauri_signing_private_key has already loaded the key from
    # TAURI_SIGNING_PRIVATE_KEY_PATH (if that was how it was supplied), so an empty
    # TAURI_SIGNING_PRIVATE_KEY means neither var was set.
    [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || missing+=("TAURI_SIGNING_PRIVATE_KEY_PATH (path to the updater key) or TAURI_SIGNING_PRIVATE_KEY (key contents)")
    if [ "${#missing[@]}" -gt 0 ]; then
        die "release mode requires these auto-injected vars to be non-empty: ${missing[*]} (do NOT export/override them — they are injected as DB env vars + mapped credentials; see docs/desktop-app.md § Shipping)"
    fi
}

# The bundle.resources map (`bundle-resources/<name>` → `<name>` for each staged
# RESOURCE_NAME) as inner JSON object members. Single source of truth so the two
# consumers — the no-version resource_config_json (also the verify loop's
# expectation) and the versioned tauri_build_config_json — can't drift, and a new
# resource is added in exactly one place. `lucidos` (the CLI) is load-bearing:
# without it cargo tauri build never copies the binary into Contents/Resources,
# so the engine can't launch the CC permission MCP server.
resource_map_json() {
    printf '%s' '"bundle-resources/lucidos-engine":"lucidos-engine","bundle-resources/lucidos-gateway":"lucidos-gateway","bundle-resources/lucidos":"lucidos","bundle-resources/frontend":"frontend","bundle-resources/postgres":"postgres","bundle-resources/sdk":"sdk","bundle-resources/system-knowhow":"system-knowhow"'
}

resource_config_json() {
    printf '%s' "{\"bundle\":{\"resources\":{$(resource_map_json)}}}"
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
        printf '%s' "{\"version\":\"$ver\",\"bundle\":{\"resources\":{$(resource_map_json)}}}"
    else
        resource_config_json
    fi
}

usage() {
    cat <<'EOF'
Usage:
  ./scripts/build-dmg.sh                 # build an unsigned local .dmg (no events)
  ./scripts/build-dmg.sh --check         # validate the staged resource contract
  ./scripts/build-dmg.sh --emit-tarball  # also emit the headless .tar.gz + .sha256
  APPLE_SIGNING_IDENTITY="Developer ID Application: …" \
  APPLE_ID=… APPLE_PASSWORD=… APPLE_TEAM_ID=… \
  TAURI_SIGNING_PRIVATE_KEY_PATH=~/.tauri/lucidos-updater.key \
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD=… \
    ./scripts/build-dmg.sh               # signed + notarized (local, no events/upload)
  ./scripts/build-dmg.sh --release-build # build + sign + notarize + STAGE (no upload)
  ./scripts/build-dmg.sh --release-attach \
    --staging-dir <dir> --upload-tag v<N.N.N> --notes-file <changelog-section>
                                         # verify staging + upload it (no rebuild)
  ./scripts/build-dmg.sh --release \
    --release-version <N.N.N> --upload-tag v<N.N.N> \
    --notes-file <changelog-section> --repo-slug <owner/repo>
                                         # one-shot (build-then-attach), back-compat

Release-mode flags:
  --release            one-shot: --release-build then --release-attach (back-compat)
  --release-build      build + sign + notarize + stage; no upload, no Release
  --release-attach     verify --staging-dir + upload it; no rebuild, no signing
  --staging-dir DIR    staging dir (required for --release-attach; default for the
                       build modes is .lucidos/release-staging/<version>)
  --release-version V  expected version; must equal the RELEASE file / manifest
  --upload-tag TAG     GitHub Release tag to attach the DMG + updater assets to
  --notes-file FILE    CHANGELOG section used as the latest.json `notes`
  --repo-slug OWNER/R  GitHub repo for the release (default lucidos-dev/lucidos)

Resumable notarization:
  The notarize stage submits with --no-wait and persists the submission id to
  .lucidos/release-state/notarize-<version>.json BEFORE it starts polling, so
  losing the waiting process costs a poll, not a rebuild. Any build-grade run that
  finds a resumable handle for its version resumes automatically.
  --defer-notarization submit to Apple, persist the resume handle, and STAGE THE
                       UNSTAPLED DMG instead of waiting for the verdict, so the
                       release can publish now (manifest records
                       notarized:false). Build-grade runs only, never with
                       --release/--release-attach. With --resume-notarize it
                       stages an ALREADY in-flight submission without polling.
                       Finish with: release.sh --attach-notarized <version>
  --allow-pending-notarization
                       --release-attach only: permit uploading a staging whose
                       DMG is not notarized yet. Passed ONLY by
                       release-to-lucidos.sh, on the branch that composed the
                       "notarization pending" banner for the Release body.
  --resume-notarize    resume deliberately: skip build + codesign + submit, poll
                       the recorded submission, then staple + stage. Refuses if
                       the DMG on disk no longer matches what was submitted or the
                       tree has moved off the commit it was built from.
  --adopt-submission U write a resume handle for submission UUID U against the
                       already-built, already-signed DMG on disk, then resume.
                       Use when a submission is in flight but its id was never
                       persisted. Implies --resume-notarize.
  Env: NOTARIZE_POLL_INTERVAL (default 30s), NOTARIZE_POLL_TIMEOUT (default
  7200s — bounds this process only; the handle outlives it),
  NOTARIZE_POLL_MAX_FAILURES (default 5 consecutive transient errors).

Headless tarball:
  --emit-tarball       ALSO emit a plain lucidos-<version>-<triple>.tar.gz (engine +
                       gateway + frontend + postgres + sdk) + a .sha256 sidecar,
                       sourced from the signed .app Resources (signatures preserved).
                       Applies to any build mode (no-op under --check / a build-less
                       --release-attach); default behavior is unchanged when absent.
                       Output: the active --staging-dir, else
                       .lucidos/release-staging/<version>/.

Updater signing key:
  TAURI_SIGNING_PRIVATE_KEY_PATH   path to the updater key (e.g.
                                   ~/.tauri/lucidos-updater.key); loaded into
                                   TAURI_SIGNING_PRIVATE_KEY (what Tauri reads).
  TAURI_SIGNING_PRIVATE_KEY        back-compat: the key contents directly (or a
                                   path Tauri auto-detects).

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

# stage_release_artifacts <staging-dir> — copy the just-built signed/notarized DMG
# + updater tarball + .sig into <staging-dir> and write manifest.json (version,
# the git HEAD of the built tree as source_commit, and a sha256 per artifact).
# This is the hand-off the verify-then-publish flow consumes: --release-attach
# (and release.sh --publish-verified) re-verify this manifest before any upload,
# so the DMG you verified is bit-for-bit the DMG that ships. No upload, no Release.
stage_release_artifacts() {
    local dir="$1"
    local app_tarball app_sig source_commit
    app_tarball="$(/usr/bin/find "$BUNDLE_DIR/macos" -name '*.app.tar.gz' 2>/dev/null | head -1 || true)"
    app_sig="$(/usr/bin/find "$BUNDLE_DIR/macos" -name '*.app.tar.gz.sig' 2>/dev/null | head -1 || true)"
    [ -n "$app_tarball" ] || die "no .app.tar.gz produced — is the updater key set (TAURI_SIGNING_PRIVATE_KEY_PATH or TAURI_SIGNING_PRIVATE_KEY)?"
    [ -n "$app_sig" ]     || die "no .app.tar.gz.sig produced — is the updater key set (TAURI_SIGNING_PRIVATE_KEY_PATH or TAURI_SIGNING_PRIVATE_KEY)?"
    source_commit="$(git -C "$REPO_ROOT" rev-parse HEAD)" \
        || die "cannot read git HEAD of $REPO_ROOT for the staging manifest"

    # Record WHAT THIS DMG WAS BUILT FROM in content terms, not just commit
    # terms. A later re-fold compares these to decide whether rebuilding could
    # change any shipped byte; without them it must (correctly) rebuild.
    # Non-fatal on failure: a missing fingerprint degrades to today's behaviour
    # (always rebuild), which is safe — losing the whole staging step over it
    # would not be.
    local build_fp recipe_fp
    build_fp="$(release_build_fingerprint_compute "$REPO_ROOT" "$source_commit" 2>/dev/null || true)"
    recipe_fp="$(release_build_recipe_fingerprint_compute "$REPO_ROOT" "$source_commit" 2>/dev/null || true)"

    rm -rf "$dir"
    mkdir -p "$dir"
    cp "$DMG_PATH" "$dir/"
    cp "$app_tarball" "$dir/"
    cp "$app_sig" "$dir/"
    RELEASE_STAGING_BUILD_FINGERPRINT="$build_fp" \
    RELEASE_STAGING_RECIPE_FINGERPRINT="$recipe_fp" \
    RELEASE_STAGING_NOTARIZED="$DMG_NOTARIZED_STATE" \
    release_staging_write_manifest "$dir" "$EFFECTIVE_VERSION" "$source_commit" \
        "$(basename "$DMG_PATH")" "$(basename "$app_tarball")" "$(basename "$app_sig")" \
        || die "failed to write the staging manifest in $dir"
    [ -n "$build_fp" ] && echo "    build fingerprint: $build_fp"
    if [ "$DMG_NOTARIZED_STATE" = "false" ]; then
        echo "    notarized: false — the DMG is signed but NOT yet stapled (deferred release)"
    fi
}

# headless_tarball_version — the version stamped into the headless tarball name.
# Release builds carry EFFECTIVE_VERSION (from RELEASE); otherwise defer to the
# shared resolver (RELEASE → tauri.conf.json → 0.0.0), the same logic
# build-headless.sh uses. Mirrors how the .dmg gets its version.
headless_tarball_version() {
    if [ -n "$EFFECTIVE_VERSION" ]; then
        printf '%s' "$EFFECTIVE_VERSION"
        return 0
    fi
    stage_runtime_version "$REPO_ROOT" "$APP_DIR"
}

# emit_headless_tarball — package the self-contained runtime tree as the headless
# lucidos-<version>-<triple>.tar.gz + .sha256 sidecar, IN ADDITION to the .app/.dmg
# (step 1 of docs/plans/2026-06-30-installer-step1-headless-tarball.md; a later install.sh
# downloads it instead of compiling). It sources from the SIGNED .app
# Contents/Resources — NOT bundle-resources/, whose copies sign_app_bundle never
# touches — so the Mach-O files in the tarball keep their Developer ID signatures.
# This reuses the already-built/-signed staging tree (no PG re-fetch, no pgvector
# recompile). Output goes to the active STAGING_DIR when a release build set one,
# else .lucidos/release-staging/<version>/; a release dir is unaffected because the
# tarball is NOT added to manifest.json (release_staging_verify ignores it).
emit_headless_tarball() {
    local resources version out_dir tar_path
    resources="$APP_PATH/Contents/Resources"
    [ -d "$resources" ] || die "headless tarball: $resources not found (no built .app to package)"
    version="$(headless_tarball_version)"
    out_dir="${STAGING_DIR:-$REPO_ROOT/.lucidos/release-staging/$version}"
    tar_path="$(headless_tarball_emit "$resources" "$out_dir" "$version" "$TARGET_TRIPLE" "${RESOURCE_NAMES[@]}")" \
        || die "failed to emit the headless tarball"
    HEADLESS_TARBALL_PATH="$tar_path"
}

# upload_staged_assets <staging-dir> — generate latest.json from the STAGED
# updater .sig + notes and attach every staged artifact (DMG + updater tarball +
# .sig + latest.json) to the GitHub Release $UPLOAD_TAG. No rebuild — the bytes
# are exactly what was staged (and, for --release-attach, already verified). This
# is the cockpit `upload` step. release-to-lucidos.sh creates the Release first;
# we only attach assets to it.
upload_staged_assets() {
    local dir="$1"
    [ -n "$UPLOAD_TAG" ] || die "release upload requires --upload-tag"
    command -v gh >/dev/null 2>&1 || die "gh CLI required to upload release artifacts (https://cli.github.com/)."

    local dmg app_tarball app_sig
    dmg="$(/usr/bin/find "$dir" -maxdepth 1 -name '*.dmg' 2>/dev/null | head -1 || true)"
    app_tarball="$(/usr/bin/find "$dir" -maxdepth 1 -name '*.app.tar.gz' 2>/dev/null | head -1 || true)"
    app_sig="$(/usr/bin/find "$dir" -maxdepth 1 -name '*.app.tar.gz.sig' 2>/dev/null | head -1 || true)"
    [ -n "$dmg" ]         || die "no staged .dmg in $dir"
    [ -n "$app_tarball" ] || die "no staged .app.tar.gz in $dir"
    [ -n "$app_sig" ]     || die "no staged .app.tar.gz.sig in $dir"

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
        "$dmg" "$app_tarball" "$app_sig" "$latest_json" \
        || die "gh release upload failed for $UPLOAD_TAG"
    rm -rf "$latest_dir"
}

# run_release_attach — the --release-attach entry point: VERIFY the staged
# artifacts against their manifest, then upload them. NO build, NO signing, NO
# rebuild. Requires --staging-dir; the version comes from the staged manifest.
run_release_attach() {
    [ -n "$STAGING_DIR_ARG" ] || die "--release-attach requires --staging-dir <dir>"
    STAGING_DIR="$STAGING_DIR_ARG"

    # Arm the failure trap so an upload error emits ReleaseStepFailed. The verify
    # below runs with CURRENT_STEP unset, so a bad manifest just exits non-zero
    # (no event) — exactly what the cockpit should see for a pre-upload refusal.
    trap on_err ERR

    # Integrity guard: refuse on missing manifest, missing artifact, or checksum
    # mismatch BEFORE touching the network.
    release_staging_verify "$STAGING_DIR" \
        || die "staging verification failed for $STAGING_DIR — refusing to attach unverified artifacts."

    EFFECTIVE_VERSION="$(release_staging_manifest_field "$STAGING_DIR" version)" \
        || die "could not read version from $STAGING_DIR/manifest.json"
    if [ -n "$RELEASE_VERSION_ARG" ] && [ "$RELEASE_VERSION_ARG" != "$EFFECTIVE_VERSION" ]; then
        die "version mismatch: --release-version '$RELEASE_VERSION_ARG' != staged manifest version '$EFFECTIVE_VERSION'."
    fi

    # A PENDING (unstapled) staging must not be uploaded from here. The
    # "notarization pending" banner is composed in release-to-lucidos.sh, on the
    # two-phase path — a direct attach would put an unnotarized DMG on a Release
    # with no warning and no dmg_pending on LucidosReleased, so the site would
    # repoint its download link at a Gatekeeper block. Refusing the flag
    # combination at parse time was not enough: nothing stopped a SEPARATE later
    # invocation pointed at the same staging dir, which is why the check belongs
    # on the manifest here rather than on the command line.
    if ! release_staging_is_notarized "$STAGING_DIR"; then
        [ "$ALLOW_PENDING_NOTARIZATION" = "1" ] || die "refusing to attach a NOT-notarized staging ($STAGING_DIR).
       That DMG has no Apple ticket yet, and this path cannot add the
       'notarization pending' banner that must accompany it. Publish a deferred
       release through the two-phase flow instead, which composes the banner and
       then passes --allow-pending-notarization here:
           scripts/release.sh --publish-verified $EFFECTIVE_VERSION
       and finish it once Apple answers:
           scripts/release.sh --attach-notarized $EFFECTIVE_VERSION"
        step "Attaching a PENDING (unstapled) DMG — the caller has published the notarization banner"
    fi

    begin_step upload "Generating latest.json and attaching the STAGED signed DMG + updater artifacts to $UPLOAD_TAG (no rebuild)."
    step "Uploading staged DMG + updater artifacts to GitHub Release $UPLOAD_TAG"
    upload_staged_assets "$STAGING_DIR"
    end_step upload "Uploaded the staged signed DMG + updater tarball + .sig + latest.json to $UPLOAD_TAG."
}

# ── Notarization (submit → persist → poll → staple) ──────────────────────────
# These live up here with the other entry-point helpers because the resume path
# runs BEFORE the build section, and a function must be defined before its call.

# notarize_credentials_present — the notarization gate, unchanged from the
# original inline condition: notarization runs only when the Apple-ID triple is
# fully set. The App Store Connect API key is a PREFERENCE WITHIN that gate (see
# notarytool_run), not an alternative to it — widening the gate here would silently
# change which local builds get notarized.
notarize_credentials_present() {
    [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]
}

# notarytool_run <subcommand> [args…] — the SINGLE credential-resolution point for
# every notarytool call (submit, info, log all take the identical credential
# flags). Callers must have passed notarize_credentials_present first.
#
# Feed the app-specific password over STDIN, never on the command line and never
# via a Keychain profile.
#
# argv is world-readable (`ps -eo command`), so `--password "$APPLE_PASSWORD"`
# exposed the password to every local process for the full duration of the
# notarization wait — often the better part of an hour. That was the original bug.
#
# The first fix reached for `notarytool store-credentials`, which is the documented
# answer but does NOT work here: the release runs as a headless subprocess with no
# GUI session, so the Security framework refuses the keychain write with "User
# interaction is not allowed" and the whole release dies at the notarize step. Do
# not reintroduce it.
#
# notarytool prompts for the password on stdin when --apple-id and --team-id are
# supplied without --password, so a plain pipe satisfies both constraints: nothing
# sensitive in argv, no keychain interaction. App Store Connect API key takes
# precedence when configured: a team-scoped credential with no Apple-ID -> team
# binding, nothing sensitive in argv (the secret is the .p8 on disk, passed by
# path). -i (issuer) is REQUIRED for Team keys, must be OMITTED for Individual
# keys -- Apple 401s a Team key submitted without it.
notarytool_run() {
    if [ -n "${APPLE_API_KEY_PATH:-}" ] && [ -n "${APPLE_API_KEY_ID:-}" ]; then
        if [ -n "${APPLE_API_ISSUER_ID:-}" ]; then
            xcrun notarytool "$@" \
                -k "$APPLE_API_KEY_PATH" -d "$APPLE_API_KEY_ID" -i "$APPLE_API_ISSUER_ID"
        else
            xcrun notarytool "$@" \
                -k "$APPLE_API_KEY_PATH" -d "$APPLE_API_KEY_ID"
        fi
    else
        printf '%s' "$APPLE_PASSWORD" | xcrun notarytool "$@" \
            --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID"
    fi
}

# notarize_announce_credentials — one line naming which credential path is in
# use, so a release log shows whether the API key or the Apple ID was picked.
# Neither the key path nor the Apple ID itself is printed.
notarize_announce_credentials() {
    if [ -n "${APPLE_API_KEY_PATH:-}" ] && [ -n "${APPLE_API_KEY_ID:-}" ]; then
        echo "    notarizing via App Store Connect API key ${APPLE_API_KEY_ID}"
    else
        echo "    notarizing via Apple ID (app-specific password on stdin)"
    fi
}

# notarize_submit <dmg> — upload WITHOUT waiting; sets NOTARIZE_SUBMISSION_ID.
# Splitting submit from wait is the whole fix: the id is known within seconds, so
# the caller can persist it before anything can kill this process.
notarize_submit() {
    local dmg="$1" out errfile
    errfile="$(mktemp -t lucidos-notarytool)"
    if ! out="$(notarytool_run submit "$dmg" --no-wait --output-format json 2>"$errfile")"; then
        local err; err="$(cat "$errfile")"; rm -f "$errfile"
        die "notarytool submit failed for $(basename "$dmg"): $err"
    fi
    rm -f "$errfile"
    # The upload already happened, so a parse failure here is the ONE way a
    # submission can reach Apple with no handle on disk. The raw reply carries the
    # id, so print it and name the recovery instead of just failing.
    NOTARIZE_SUBMISSION_ID="$(printf '%s' "$out" | release_notarize_json_field id)" \
        || die "could not parse a submission id out of notarytool's reply. The upload succeeded, so pick the id out of this reply and resume with --adopt-submission <uuid>: $out"
    [ -n "$NOTARIZE_SUBMISSION_ID" ] \
        || die "notarytool submit returned an empty submission id. The upload succeeded, so find the id with 'xcrun notarytool history' and resume with --adopt-submission <uuid>: $out"
}

# notarize_poll <submission-id> — block until Apple's verdict; sets NOTARIZE_STATUS.
# Three outcomes are distinguished, because they need different human responses:
#   • a terminal status                 → return, caller acts on it
#   • an id Apple doesn't recognise     → the handle is stale; a fresh submit is
#                                         required (never silently re-submit)
#   • a transient failure               → retry, up to NOTARIZE_POLL_MAX_FAILURES
#                                         consecutively (a network blip must not
#                                         throw away a 40-minute wait)
notarize_poll() {
    local id="$1"
    local waited=0 fails=0 out status lowered err errfile
    errfile="$(mktemp -t lucidos-notarytool)"
    while :; do
        if out="$(notarytool_run info "$id" --output-format json 2>"$errfile")"; then
            fails=0
            status="$(printf '%s' "$out" | release_notarize_json_field status 2>/dev/null || true)"
            if [ -z "$status" ]; then
                rm -f "$errfile"
                die "notarytool info returned no status for submission $id: $out"
            fi
            if [ "$status" != "In Progress" ]; then
                rm -f "$errfile"
                NOTARIZE_STATUS="$status"
                return 0
            fi
        else
            err="$(cat "$errfile")"
            [ -n "$err" ] || err="$out"
            # Only Apple's "no such submission" phrasings count as terminal, and
            # each must name the submission or the 404 status. Two near-misses this
            # deliberately avoids: a bare "404" (the error echoes the submission id,
            # and a hex UUID can contain 404), and a bare "unable to find" (that is
            # also `xcrun: error: unable to find utility "notarytool"` — a tooling
            # problem). Either would tell the operator to delete a perfectly good
            # resume handle. An unrecognised wording just retries and then dies
            # showing the raw error, which is the safe way to be wrong.
            lowered="$(printf '%s %s' "$out" "$err" | tr '[:upper:]' '[:lower:]')"
            case "$lowered" in
                *"status code: 404"*|*"submission does not exist"*|*"submission id not found"*|*"does not exist or does not belong"*)
                    rm -f "$errfile"
                    die "Apple does not recognise submission $id — the notarize state is stale. Delete $NOTARIZE_STATE_FILE and re-run the build to submit afresh. ($err)"
                    ;;
            esac
            fails=$((fails + 1))
            if [ "$fails" -ge "$NOTARIZE_POLL_MAX_FAILURES" ]; then
                rm -f "$errfile"
                die "notarytool info failed $fails times in a row for submission $id: $err"
            fi
            echo "    notarytool info failed (attempt $fails/$NOTARIZE_POLL_MAX_FAILURES) — retrying in ${NOTARIZE_POLL_INTERVAL}s: $err"
        fi

        if [ "$waited" -ge "$NOTARIZE_POLL_TIMEOUT" ]; then
            rm -f "$errfile"
            die "submission $id is still In Progress after ${waited}s. Nothing is lost — the resume handle is at $NOTARIZE_STATE_FILE; pick it back up with: scripts/build-dmg.sh --release-build --resume-notarize"
        fi
        sleep "$NOTARIZE_POLL_INTERVAL"
        waited=$((waited + NOTARIZE_POLL_INTERVAL))
        printf '    still In Progress after %dm%02ds (submission %s)\n' \
            "$((waited / 60))" "$((waited % 60))" "$id"
    done
}

# notarize_print_log <submission-id> — dump Apple's notary log. Only called on a
# rejection, where the log is the ONLY explanation of what failed; never allowed
# to mask the rejection itself by failing the run.
notarize_print_log() {
    local id="$1"
    echo "    fetching the notary log for $id"
    notarytool_run log "$id" || echo "    (could not fetch the notary log for $id)"
}

# notarize_await_verdict <submission-id> — poll to a terminal status and act:
# Accepted continues to stapling; anything else prints the notary log and fails
# loud WITHOUT stapling or staging (a rejected build must never reach a staging
# manifest, which is what --publish-verified would go on to ship).
notarize_await_verdict() {
    local id="$1"
    notarize_poll "$id"
    if [ "$NOTARIZE_STATUS" = "Accepted" ]; then
        echo "    notarization Accepted for submission $id"
        return 0
    fi
    notarize_print_log "$id"
    die "notarization $NOTARIZE_STATUS for submission $id — refusing to staple or stage. The notary log above says why."
}

# staple_idempotent <path> — attach the notarization ticket, tolerating an
# artifact that already carries one. Re-stapling is normal on the resume path (the
# run that would have stapled is exactly the run that died), so a second staple
# must not fail the release: fall back to `stapler validate` and treat an
# already-attached ticket as success.
staple_idempotent() {
    local path="$1"
    if xcrun stapler staple "$path"; then
        return 0
    fi
    if xcrun stapler validate "$path" >/dev/null 2>&1; then
        echo "    $(basename "$path") already carries a valid notarization ticket — nothing to staple."
        return 0
    fi
    die "stapler staple failed for $path and the artifact carries no valid ticket."
}

# assert_dmg_is_the_submitted_bytes — refuse to staple a DMG that is no longer
# the file Apple scanned.
#
# THE 2026-07-28 ORPHANED-POLLER BUG. build-dmg.sh writes the DMG to a FIXED path
# (target/release/bundle/dmg/Lucidos_<version>_aarch64.dmg), so a rebuild
# overwrites the exact file an in-flight submission was for. That day three
# pollers were alive at once and two of them were waiting on submissions whose
# DMG bytes no longer existed — had those verdicts returned, each would have
# stapled a ticket issued for one set of bytes onto a different set.
#
# release_notarize_resumable() checks this on the RESUME path. This is the same
# assertion on the FRESH-BUILD path, which had none: submit → (long wait) →
# staple has exactly the same window, because a concurrent build can overwrite
# the DMG while this process sits in notarize_poll. Cheap (one sha256 of a file
# already in the page cache) against a silent mis-staple.
assert_dmg_is_the_submitted_bytes() {
    local expected="$1" actual pinned
    [ -n "$expected" ] || return 0

    # If the fixed path was overwritten by a concurrent build, the pinned copy
    # still holds the exact bytes Apple scanned — recover from it instead of
    # throwing away a completed notarization.
    if [ ! -f "$DMG_PATH" ] || [ "$(release_staging_sha256 "$DMG_PATH" 2>/dev/null || true)" != "$expected" ]; then
        pinned="${NOTARIZE_PINNED_DMG:-}"
        if [ -z "$pinned" ]; then
            pinned="$(/usr/bin/find "$REPO_ROOT/.lucidos/notarize-submissions/${EFFECTIVE_VERSION:-unversioned}/${expected:0:12}" \
                -maxdepth 1 -name '*.dmg' 2>/dev/null | head -1 || true)"
        fi
        if [ -n "$pinned" ] && [ -f "$pinned" ] \
           && [ "$(release_staging_sha256 "$pinned" 2>/dev/null || true)" = "$expected" ]; then
            echo "    NOTE: $DMG_PATH no longer holds the submitted bytes (a rebuild replaced it)."
            echo "          Recovering the notarized bytes from the pin: $pinned"
            cp -f "$pinned" "$DMG_PATH" \
                || die "could not restore the submitted DMG from its pin at $pinned"
        fi
    fi

    [ -f "$DMG_PATH" ] || die "the DMG vanished while Apple was notarizing it: $DMG_PATH"
    actual="$(release_staging_sha256 "$DMG_PATH")" \
        || die "could not re-hash $DMG_PATH before stapling"
    if [ "$actual" != "$expected" ]; then
        die "REFUSING TO STAPLE: $DMG_PATH is not the file that was submitted.
       submitted: $expected
       on disk:   $actual
       Another build overwrote the DMG while this submission was in flight.
       Stapling now would attach a notarization ticket issued for different
       bytes. Rebuild, or resume the submission from the tree that built it."
    fi
}

# staple_notarized_artifacts [<expected-dmg-sha256>] — staple the DMG and (when
# present) the .app. When the caller knows the sha256 that was submitted it MUST
# pass it: stapling different bytes than Apple scanned is the failure mode this
# guards (see assert_dmg_is_the_submitted_bytes).
staple_notarized_artifacts() {
    assert_dmg_is_the_submitted_bytes "${1:-}"
    step "Stapling the notarization ticket"
    staple_idempotent "$DMG_PATH"
    if [ -n "$APP_PATH" ] && [ -e "$APP_PATH" ]; then
        staple_idempotent "$APP_PATH"
    else
        echo "    (no .app on disk to staple — the DMG carries the ticket that matters)"
    fi
}

# notarize_pin_submitted_dmg <sha256> — hardlink the DMG about to be submitted
# into .lucidos/notarize-submissions/<version>/<sha12>/ and set
# NOTARIZE_PINNED_DMG to that path.
#
# WHY: the release DMG is written to a fixed, version-scoped path, so EVERY
# rebuild of the same version overwrites it. An in-flight notarization refers to
# the BYTES, not the path — so a rebuild silently orphans the submission, and the
# poller waiting on it is now watching a file that no longer contains what Apple
# scanned.
#
# WHY A CLONE AND NOT A HARDLINK. A hardlink is the obvious zero-cost pin and it
# is WRONG here: it is a second name for the SAME inode, so anything that writes
# the DMG IN PLACE — `codesign` rewriting the signature, a truncating `>` — is
# seen through both names and silently corrupts the pin. (The test suite proves
# this: an in-place write mutates a hardlinked pin while leaving a cloned one
# intact.) `cp -c` requests an APFS clonefile: copy-on-write, so it costs no disk
# until one side diverges, and an in-place write to the original allocates new
# blocks instead of touching the pinned copy. On a non-APFS volume the -c fails
# and we fall back to a plain copy — correct, just not free.
#
# Best-effort by design: if the pin cannot be created the build proceeds
# unpinned. The sha assertion before stapling is the correctness guarantee (it
# refuses rather than mis-staples); the pin is what additionally lets a poller
# RECOVER the right bytes instead of losing a completed notarization.
notarize_pin_submitted_dmg() {
    local sha="$1" pin_dir
    [ -n "$sha" ] || return 0
    pin_dir="$REPO_ROOT/.lucidos/notarize-submissions/${EFFECTIVE_VERSION:-unversioned}/${sha:0:12}"
    if ! mkdir -p "$pin_dir" 2>/dev/null; then
        echo "    NOTE: could not create $pin_dir — submitting unpinned."
        return 0
    fi
    NOTARIZE_PINNED_DMG="$pin_dir/$(basename "$DMG_PATH")"
    if [ -f "$NOTARIZE_PINNED_DMG" ]; then
        echo "    pinned (already): $NOTARIZE_PINNED_DMG"
        return 0
    fi
    if cp -c "$DMG_PATH" "$NOTARIZE_PINNED_DMG" 2>/dev/null \
       || cp "$DMG_PATH" "$NOTARIZE_PINNED_DMG" 2>/dev/null; then
        echo "    pinned submitted bytes: $NOTARIZE_PINNED_DMG"
    else
        echo "    NOTE: could not pin $DMG_PATH — submitting unpinned."
        NOTARIZE_PINNED_DMG=""
    fi
}

# notarize_submit_and_persist — submit and durably record the handle, stopping
# short of the wait. Split out of notarize_submit_and_wait because a DEFERRED
# release needs exactly this half: the submission is with Apple and recoverable
# from disk, and the build is free to stage and publish without the verdict.
notarize_submit_and_persist() {
    local sha commit submitted_at
    sha="$(release_staging_sha256 "$DMG_PATH")" \
        || die "could not hash $DMG_PATH for the notarize resume handle"
    NOTARIZE_SUBMITTED_SHA="$sha"
    commit="$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || true)"
    submitted_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

    # PIN THE SUBMITTED BYTES before handing them to Apple. The DMG lives at a
    # FIXED path that the next build overwrites, so the file the notary is
    # scanning can silently become different bytes mid-flight (2026-07-28: three
    # concurrent pollers, two of them orphaned this way). The pin is a hardlink
    # into an immutable per-submission directory — same inode, so it costs no
    # disk and no copy time, and a later `mv -f` onto the fixed path replaces the
    # DIRECTORY ENTRY without touching the pinned inode.
    notarize_pin_submitted_dmg "$sha"

    step "Submitting $(basename "$DMG_PATH") to the Apple notary service"
    notarize_submit "$DMG_PATH"

    # Persist BEFORE the first poll. Everything above this line is expensive and
    # already on disk; from here on, losing this process costs a poll rather than a
    # full rebuild (the 2026-07-28 incident, where the id died with the waiter).
    release_notarize_write_state "$NOTARIZE_STATE_FILE" "$NOTARIZE_SUBMISSION_ID" \
        "$DMG_PATH" "$EFFECTIVE_VERSION" "$sha" "$commit" "$submitted_at" \
        || die "could not persist the notarize resume handle to $NOTARIZE_STATE_FILE"
    echo "    submission $NOTARIZE_SUBMISSION_ID — resume handle: $NOTARIZE_STATE_FILE"
}

# notarize_submit_and_wait — the fresh-build path: submit, PERSIST the resume
# handle before any waiting, then poll for the verdict.
notarize_submit_and_wait() {
    notarize_submit_and_persist
    notarize_await_verdict "$NOTARIZE_SUBMISSION_ID"
}

# notarize_adopt_submission <head-commit> — write a resume handle for a submission
# that is ALREADY in flight with Apple but whose id was never persisted (it only
# ever existed in the dead process's stdout). The DMG on disk is taken to be the
# one that was submitted; the poll below verifies the id with Apple, and the
# checksum recorded here is what every later resume is gated on.
notarize_adopt_submission() {
    local head_commit="$1" dmg_dir dmg sha count
    dmg_dir="$REPO_ROOT/target/release/bundle/dmg"
    # Exclude refresh_dmg_payload's intermediates: a run killed mid-refresh can
    # leave a .rw.dmg / .zlib.dmg behind, and adopting one of those would record a
    # checksum for bytes Apple never saw. Require exactly one real candidate.
    dmg="$(/usr/bin/find "$dmg_dir" -maxdepth 1 -name '*.dmg' \
        ! -name '*.rw.dmg' ! -name '*.zlib.dmg' 2>/dev/null | sort || true)"
    [ -n "$dmg" ] \
        || die "--adopt-submission found no built .dmg under $dmg_dir — adoption needs the signed DMG that was submitted still on disk."
    count="$(printf '%s\n' "$dmg" | wc -l | tr -d '[:space:]')"
    [ "$count" = "1" ] \
        || die "--adopt-submission found $count candidate DMGs under $dmg_dir; cannot tell which one was submitted:
$dmg"
    case "$(basename "$dmg")" in
        *"_${EFFECTIVE_VERSION}_"*) ;;
        *) die "--adopt-submission: the on-disk DMG '$(basename "$dmg")' does not carry version '$EFFECTIVE_VERSION' — refusing to adopt a submission for a different build." ;;
    esac
    sha="$(release_staging_sha256 "$dmg")" || die "could not hash $dmg"
    step "Adopting in-flight submission $ADOPT_SUBMISSION for $(basename "$dmg")"
    release_notarize_write_state "$NOTARIZE_STATE_FILE" "$ADOPT_SUBMISSION" "$dmg" \
        "$EFFECTIVE_VERSION" "$sha" "$head_commit" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        || die "could not write the notarize resume handle to $NOTARIZE_STATE_FILE"
    echo "    wrote $NOTARIZE_STATE_FILE (submitted_at records the adoption time)"
}

# run_notarize_resume — pick a lost notarization back up. NO build, NO codesign,
# NO re-submit: the DMG is already signed on disk and already with Apple, so this
# polls for the verdict, staples, and runs the same finalize tail a fresh build
# does. This is what makes a Phase A survive losing the process waiting on Apple.
run_notarize_resume() {
    local head_commit submission_id submitted_at

    [ -n "$EFFECTIVE_VERSION" ] \
        || die "resuming notarization needs a version — no RELEASE file at $REPO_ROOT/RELEASE."
    head_commit="$(git -C "$REPO_ROOT" rev-parse HEAD)" \
        || die "cannot read git HEAD of $REPO_ROOT to verify the resume handle."

    if [ -n "$ADOPT_SUBMISSION" ]; then
        notarize_adopt_submission "$head_commit"
    fi

    release_notarize_resumable "$NOTARIZE_STATE_FILE" "$head_commit" \
        || die "cannot resume notarization for $EFFECTIVE_VERSION (see the reason above)."

    submission_id="$(release_notarize_field "$NOTARIZE_STATE_FILE" submission_id)" \
        || die "could not read the submission id from $NOTARIZE_STATE_FILE"
    # Mirror it into the global the closing report reads, so a deferred resume
    # names the submission that is still with Apple instead of printing a blank.
    NOTARIZE_SUBMISSION_ID="$submission_id"
    submitted_at="$(release_notarize_field "$NOTARIZE_STATE_FILE" submitted_at)" \
        || die "could not read the submit time from $NOTARIZE_STATE_FILE"
    DMG_PATH="$(release_notarize_field "$NOTARIZE_STATE_FILE" dmg_path)" \
        || die "could not read the DMG path from $NOTARIZE_STATE_FILE"
    BUNDLE_DIR="$REPO_ROOT/target/release/bundle"
    # Staging pairs the recorded DMG with the .app.tar.gz + .sig found under
    # BUNDLE_DIR. Those must be the same build's artifacts, so refuse a handle
    # whose DMG lives somewhere else — otherwise a manifest could describe a DMG
    # from one build and updater artifacts from another.
    case "$DMG_PATH" in
        "$BUNDLE_DIR/dmg/"*) ;;
        *) die "the resume handle records a DMG outside this tree's bundle dir ($DMG_PATH is not under $BUNDLE_DIR/dmg/) — resume from the tree that built it." ;;
    esac
    APP_PATH="$(/usr/bin/find "$BUNDLE_DIR/macos" -name '*.app' 2>/dev/null | head -1 || true)"

    # A DEFERRED resume does not poll at all: the submission stays in flight and
    # the already-built, already-signed DMG is staged unstapled so the release can
    # publish. This is the path that rescues a Phase A whose verdict is hours
    # away — the build is on disk, only Apple is missing.
    #
    # It is checked BEFORE the credential gate on purpose. This branch makes no
    # network call: it re-hashes local bytes and stages them. Demanding Apple
    # credentials for that would block the rescue in exactly the situation it
    # exists for — a stuck or expired-credential release — for a check nothing
    # below it uses. Validate what you use; the polling branch validates its own.
    if [ "$DEFER_NOTARIZATION" = "1" ]; then
        step "Staging the in-flight submission $submission_id without waiting (--defer-notarization)"
        echo "    dmg:    $DMG_PATH"
        echo "    handle: $NOTARIZE_STATE_FILE"
        assert_dmg_is_the_submitted_bytes \
            "$(release_notarize_field "$NOTARIZE_STATE_FILE" dmg_sha256 2>/dev/null || true)"
        DMG_NOTARIZED_STATE="false"
        finalize_release_artifacts
        return 0
    fi

    notarize_credentials_present \
        || die "resuming notarization needs APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID to ask Apple about submission $submission_id."

    begin_step notarize "Resuming notarization of submission $submission_id (submitted $submitted_at) — polling for the verdict, then stapling. No rebuild."
    step "Resuming notarization of $submission_id, submitted $submitted_at"
    echo "    dmg:    $DMG_PATH"
    echo "    handle: $NOTARIZE_STATE_FILE"
    notarize_announce_credentials
    notarize_await_verdict "$submission_id"
    # The resume gate already hashed the DMG, but the poll above can run for
    # hours — re-assert against the recorded sha so a build that landed DURING
    # the wait cannot slip different bytes under the staple.
    staple_notarized_artifacts "$(release_notarize_field "$NOTARIZE_STATE_FILE" dmg_sha256 2>/dev/null || true)"
    end_step notarize "Notarized + stapled the DMG and the .app (resumed submission $submission_id)."

    finalize_release_artifacts
}

# finalize_release_artifacts — the shared post-notarize tail: stage (build-grade
# runs), drop the spent resume handle, emit the optional headless tarball, upload
# (one-shot --release), and print the report. BOTH the fresh build and the resume
# path end here, so the two cannot drift.
finalize_release_artifacts() {
    # ── 6b. stage artifacts (verify-then-publish hand-off) ───────────────────
    # In any release-grade BUILD (--release-build or --release) the
    # signed/notarized artifacts are staged with a checksum manifest so they can be
    # verified and then published WITHOUT a rebuild. --release-build stops after
    # staging; --release continues to the upload below against this same staging
    # dir (the old one-shot behavior, byte-for-byte: build → stage → upload now).
    if [ "$DO_BUILD" = "1" ]; then
        STAGING_DIR="${STAGING_DIR_ARG:-$REPO_ROOT/.lucidos/release-staging/$EFFECTIVE_VERSION}"
        step "Staging release artifacts → $STAGING_DIR"
        stage_release_artifacts "$STAGING_DIR"
    fi

    # The resume handle has done its job: notarization is complete and, for a
    # build-grade run, the artifacts are staged. Drop it so a later run for this
    # version builds afresh instead of resuming a finished release. A no-op when
    # notarization was skipped (unsigned local build) — no handle was ever written.
    #
    # NOT on a deferred release: there the submission is still with Apple, and
    # this handle is the ONLY record of its id. Clearing it here would strand the
    # published DMG unstaplable — the attach step could not find the submission,
    # and the only recovery would be a rebuild, which is the entire cost this
    # feature exists to avoid. The same goes for the pin: it protects the
    # submitted bytes for as long as the submission is in flight, which is now
    # past the end of this process.
    if [ "$DMG_NOTARIZED_STATE" = "false" ]; then
        echo "    keeping the resume handle + submitted-bytes pin (submission still in flight)"
    else
        release_notarize_clear "$NOTARIZE_STATE_FILE"

        # The pin exists to protect bytes while a submission is IN FLIGHT. Staging
        # has copied the stapled DMG into the staging dir under a checksum manifest,
        # so the pin's job is done — drop this version's pins rather than growing a
        # 67MB-per-build graveyard under .lucidos/.
        if [ "$DO_BUILD" = "1" ] && [ -n "${EFFECTIVE_VERSION:-}" ]; then
            rm -rf "$REPO_ROOT/.lucidos/notarize-submissions/$EFFECTIVE_VERSION"
            NOTARIZE_PINNED_DMG=""
        fi
    fi

    # ── 6b2. headless tarball (opt-in --emit-tarball) ────────────────────────
    # Emit the plain headless lucidos-<version>-<triple>.tar.gz + .sha256 IN
    # ADDITION to the .app/.dmg, sourced from the signed .app Resources so the
    # Mach-O files stay signed. Gated entirely on --emit-tarball, so default
    # behavior is unchanged. Not a cockpit step (no ReleaseStep* event); when
    # STAGING_DIR is set it lands alongside the staged DMG without entering
    # manifest.json (so verify/upload are unaffected).
    if [ "$EMIT_TARBALL" = "1" ]; then
        step "Emitting headless tarball (lucidos-<version>-$TARGET_TRIPLE.tar.gz + .sha256)"
        emit_headless_tarball
    fi

    # ── 6c. upload staged assets (one-shot --release only) ───────────────────
    # --release-build skips this (DO_ATTACH=0) and leaves the staging in place for
    # a later --release-attach / release.sh --publish-verified — the whole point of
    # the build-once / verify-first flow.
    if [ "$DO_ATTACH" = "1" ]; then
        begin_step upload "Generating latest.json and attaching the signed DMG + updater artifacts to $UPLOAD_TAG."
        step "Uploading staged DMG + updater artifacts to GitHub Release $UPLOAD_TAG"
        upload_staged_assets "$STAGING_DIR"
        end_step upload "Uploaded the signed DMG + updater tarball + .sig + latest.json to $UPLOAD_TAG."
    fi

    print_build_report
}

# print_build_report — the closing summary of a build-grade run.
print_build_report() {
    step "Done"
    echo "  .dmg:  $DMG_PATH"
    [ -n "$APP_PATH" ] && echo "  .app:  $APP_PATH"
    local updater_sig
    updater_sig="$(/usr/bin/find "$BUNDLE_DIR/macos" -name '*.app.tar.gz.sig' 2>/dev/null | head -1 || true)"
    if [ -n "$updater_sig" ]; then
        echo "  updater artifacts: $(dirname "$updater_sig")/*.app.tar.gz(.sig)"
    else
        echo "  (no updater .sig — set TAURI_SIGNING_PRIVATE_KEY_PATH to the updater key to emit signed update artifacts.)"
    fi
    if [ -n "$HEADLESS_TARBALL_PATH" ]; then
        echo "  headless tarball: $HEADLESS_TARBALL_PATH (+ .sha256)"
    fi
    if [ "$DO_BUILD" = "1" ] && [ -n "$STAGING_DIR" ]; then
        echo "  staged:  $STAGING_DIR (manifest.json + .dmg + .app.tar.gz + .sig)"
        if [ "$DMG_NOTARIZED_STATE" = "false" ]; then
            echo "  DMG:     signed, NOT notarized — submission $NOTARIZE_SUBMISSION_ID still with Apple"
        fi
        if [ "$DO_ATTACH" != "1" ]; then
            echo ""
            echo "  → Verify this DMG, then publish the SAME artifacts (no rebuild) with:"
            echo "      scripts/release.sh --publish-verified $EFFECTIVE_VERSION"
            if [ "$DMG_NOTARIZED_STATE" = "false" ]; then
                # Deliberately NOT offering the direct --release-attach here: it
                # refuses a pending staging (only release-to-lucidos.sh may attach
                # one, after composing the banner), so printing it would advertise
                # a command that is guaranteed to fail.
                echo ""
                echo "    That publish ships the DMG with a 'notarization pending' banner and"
                echo "    leaves the site's Download-for-Mac link on the previous release."
                echo "    When Apple returns the verdict, staple + swap the asset with:"
                echo "        scripts/release.sh --attach-notarized $EFFECTIVE_VERSION"
            else
                echo "    or directly:  scripts/build-dmg.sh --release-attach \\"
                echo "                    --staging-dir '$STAGING_DIR' --upload-tag <tag> --notes-file <file>"
            fi
        fi
    elif [ -z "$STAGING_DIR" ]; then
        echo "  → upload the .dmg, .app.tar.gz, .sig and a latest.json to a GitHub Release"
        echo "    (plugins.updater.endpoints in tauri.conf.json points at latest.json)."
    fi
}

DO_CHECK=0
while [ $# -gt 0 ]; do
    case "$1" in
        --check)           DO_CHECK=1; shift ;;
        -h|--help)         usage; exit 0 ;;
        --release)         RELEASE_MODE=1; DO_BUILD=1; DO_ATTACH=1; shift ;;
        --release-build)   RELEASE_MODE=1; DO_BUILD=1; shift ;;
        --release-attach)  RELEASE_MODE=1; DO_ATTACH=1; shift ;;
        --staging-dir)     [ $# -ge 2 ] || die "--staging-dir requires an argument"; STAGING_DIR_ARG="$2"; shift 2 ;;
        --release-version) [ $# -ge 2 ] || die "--release-version requires an argument"; RELEASE_VERSION_ARG="$2"; shift 2 ;;
        --upload-tag)      [ $# -ge 2 ] || die "--upload-tag requires an argument"; UPLOAD_TAG="$2"; shift 2 ;;
        --notes-file)      [ $# -ge 2 ] || die "--notes-file requires an argument"; NOTES_FILE="$2"; shift 2 ;;
        --repo-slug)       [ $# -ge 2 ] || die "--repo-slug requires an argument"; REPO_SLUG="$2"; shift 2 ;;
        --emit-tarball)    EMIT_TARBALL=1; shift ;;
        --resume-notarize) DO_RESUME_NOTARIZE=1; shift ;;
        --defer-notarization) DEFER_NOTARIZATION=1; shift ;;
        --allow-pending-notarization) ALLOW_PENDING_NOTARIZATION=1; shift ;;
        --adopt-submission)
            [ $# -ge 2 ] || die "--adopt-submission requires a notary submission UUID"
            ADOPT_SUBMISSION="$2"; DO_RESUME_NOTARIZE=1; shift 2 ;;
        *)                 die "unknown argument: $1" ;;
    esac
done

if [ "$DO_CHECK" = "1" ]; then
    check_resource_contract
    exit 0
fi

# Shape-check the adopted id up front so a typo fails here rather than after a
# credential round-trip to Apple.
if [ -n "$ADOPT_SUBMISSION" ]; then
    release_notarize_valid_submission_id "$ADOPT_SUBMISSION" \
        || die "--adopt-submission expects a notary submission UUID (8-4-4-4-12 hex), got '$ADOPT_SUBMISSION'"
fi

# --release-attach uploads artifacts a prior build already notarized and staged;
# there is no notarization left to resume, so the combination is a mistake worth
# naming rather than silently ignoring.
if [ "$DO_RESUME_NOTARIZE" = "1" ] && [ "$DO_ATTACH" = "1" ] && [ "$DO_BUILD" = "0" ]; then
    die "--resume-notarize / --adopt-submission cannot be combined with --release-attach (attach publishes an already-notarized, already-staged build)."
fi

# --defer-notarization changes what a BUILD does after submitting, so it is
# meaningless without one — and it must never reach the one-shot --release, whose
# upload happens in this same process with no way to compose the "notarization
# pending" release notes (that lives in release-to-lucidos.sh, on the two-phase
# path). Refusing here keeps "an unnotarized DMG can only ship WITH its banner"
# a property of the code rather than of the operator's memory.
if [ "$DEFER_NOTARIZATION" = "1" ]; then
    [ "$DO_BUILD" = "1" ] \
        || die "--defer-notarization only applies to a build (--release-build); there is nothing to defer otherwise."
    [ "$DO_ATTACH" != "1" ] \
        || die "--defer-notarization cannot be combined with --release / --release-attach. A deferred DMG must be published through the two-phase flow, which is what adds the 'notarization pending' banner: release.sh --verify-build --defer-notarization, then --publish-verified."
fi

# ── --release-attach (no build): verify the staged artifacts and upload them ──
# This path does NO build, NO codesign, NO notarize — it only attaches artifacts
# a prior --release-build already produced + verified. It therefore needs neither
# the Apple/Tauri signing creds nor the Darwin/cargo/tauri/npm build tooling; it
# only needs `gh` + a valid staging dir. Handled before the build preamble so the
# manifest guard fails fast offline.
if [ "$DO_ATTACH" = "1" ] && [ "$DO_BUILD" = "0" ]; then
    run_release_attach
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

# The notarize resume handle is keyed by version, so it resolves as soon as the
# version does. Empty only for an unsigned local build with no RELEASE file, where
# nothing notarizes and release_notarize_clear is a no-op.
if [ -n "$EFFECTIVE_VERSION" ]; then
    NOTARIZE_STATE_FILE="$(release_notarize_state_path "$REPO_ROOT" "$EFFECTIVE_VERSION")"
fi

# Now that EFFECTIVE_VERSION + RELEASE_MODE are known, arm the failure trap so any
# stage error emits ReleaseStepFailed for the in-flight step.
trap on_err ERR

# Resolve the Tauri updater signing key before any credential assertion or build:
# TAURI_SIGNING_PRIVATE_KEY_PATH (the self-documenting var holding the key FILE
# PATH, e.g. ~/.tauri/lucidos-updater.key) is loaded into TAURI_SIGNING_PRIVATE_KEY
# — the only name Tauri's bundler reads — so `cargo tauri build` below emits the
# signed .app.tar.gz.sig. If only the legacy TAURI_SIGNING_PRIVATE_KEY is set
# (contents, or a path Tauri auto-detects), it is left untouched. A PATH that
# doesn't resolve to a readable, non-empty file fails loud here. Harmless when
# neither var is set (unsigned local build).
resolve_tauri_signing_private_key \
    || die "could not load the Tauri updater key from TAURI_SIGNING_PRIVATE_KEY_PATH (see the error above)"

# Release mode relies solely on auto-injected creds — assert before any work so a
# missing var fails loud instead of silently skipping notarization.
if [ "$RELEASE_MODE" = "1" ]; then
    assert_release_credentials
fi

[ "$(uname -s)" = "Darwin" ] || die "build-dmg.sh builds the macOS bundle and must run on a Mac."

# Resolve the theseus-rs relocatable-binary triple for this host (shared with the
# Linux/headless path via stage_runtime.sh). On a Mac this resolves to
# <arch>-apple-darwin; an explicit TARGET_TRIPLE still wins. Resolved BEFORE the
# resume branch because a resumed run can still be asked for --emit-tarball, which
# names the triple; it is a pure uname map, not part of the build toolchain.
if [ -z "${TARGET_TRIPLE:-}" ]; then
    TARGET_TRIPLE="$(stage_runtime_host_triple)" \
        || die "could not resolve the target triple for $(uname -s)/$(uname -m)"
fi

# ── Resume a lost notarization (no build, no codesign, no re-submit) ─────────
# Placed after the Darwin check but BEFORE the cargo/tauri/npm checks: a resume
# only needs xcrun, and demanding a full build toolchain to finish a build that
# already happened would be a pointless way to fail.
if [ "$DO_RESUME_NOTARIZE" = "1" ]; then
    run_notarize_resume
    exit 0
fi

# Automatic detection: a build-grade run that finds a resumable handle for its
# version picks it up instead of spending a full rebuild. A handle that exists but
# ISN'T resumable (rebuilt DMG, moved tree) is not an error here — say why and
# build afresh, which is what the operator asked for.
if [ "$DO_BUILD" = "1" ] && [ -n "$NOTARIZE_STATE_FILE" ] && [ -f "$NOTARIZE_STATE_FILE" ]; then
    HEAD_COMMIT="$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || true)"
    if RESUME_REFUSAL="$(release_notarize_resumable "$NOTARIZE_STATE_FILE" "$HEAD_COMMIT" 2>&1)"; then
        step "Found a resumable notarization for $EFFECTIVE_VERSION — resuming instead of rebuilding"
        DO_RESUME_NOTARIZE=1
        run_notarize_resume
        exit 0
    fi
    echo "NOTE: $NOTARIZE_STATE_FILE exists but is not resumable — building afresh."
    printf '%s\n' "$RESUME_REFUSAL" | sed 's/^/      /'
fi

command -v cargo >/dev/null || die "cargo not found — install Rust (https://rustup.rs)."
if ! cargo tauri --version >/dev/null 2>&1; then
    die "tauri CLI not found. Install it:  cargo install tauri-cli --locked"
fi
command -v npm >/dev/null || die "npm not found — install Node.js."

begin_step build "Compiling engine + gateway + app, fetching PostgreSQL $PG_VERSION + pgvector, running cargo tauri build (.app + .dmg)."

# Staging steps 1–4 are the platform-agnostic spine shared with the Linux/headless
# path; they live in scripts/lib/stage_runtime.sh so the recipe exists once.

# ── 1. frontend ─────────────────────────────────────────────────────────────
step "Building frontend (dist/)"
stage_runtime_build_frontend "$REPO_ROOT" "$APP_DIR" || die "frontend build failed"

# ── 2. gateway + engine + cli (release) ─────────────────────────────────────
# Build engine + gateway + the `lucidos` CLI — all three are bundled. The CLI is
# load-bearing: the engine launches the CC permission-prompt MCP server via the
# sibling `lucidos` binary (find_lucidos_cli_dir), so a build that omits it breaks
# every coding-agent thread in the packaged app on its first tool call.
step "Building gateway + engine + cli (release)"
stage_runtime_build_binaries "$REPO_ROOT" lucidos-engine lucidos-gateway lucidos-cli \
    || die "gateway + engine + cli build failed"
ENGINE_BIN="$REPO_ROOT/target/release/lucidos-engine"
GATEWAY_BIN="$REPO_ROOT/target/release/lucidos-gateway"
CLI_BIN="$REPO_ROOT/target/release/lucidos"
[ -x "$ENGINE_BIN" ] || die "engine binary not found at $ENGINE_BIN"
[ -x "$GATEWAY_BIN" ] || die "gateway binary not found at $GATEWAY_BIN"
[ -x "$CLI_BIN" ] || die "lucidos CLI binary not found at $CLI_BIN"

# ── 3. relocatable PostgreSQL + pgvector ────────────────────────────────────
# Mirrors scripts/prototype/desktop-pg-pgvector-spike.sh (proven recipe). On macOS
# stage_runtime_fetch_postgres applies the PG_SYSROOT override; on Linux it uses
# system gcc.
step "Fetching relocatable PostgreSQL $PG_VERSION + building pgvector $PGVECTOR_VERSION ($TARGET_TRIPLE)"
PG_PREFIX="$(stage_runtime_fetch_postgres "$PG_VERSION" "$PGVECTOR_VERSION" "$TARGET_TRIPLE" "$REPO_ROOT/.lucidos/dmg-build/pg")" \
    || die "failed to fetch/compile relocatable PostgreSQL + pgvector for $TARGET_TRIPLE"

# ── 4. stage resources ──────────────────────────────────────────────────────
step "Staging bundle resources → $STAGE"
stage_runtime_assemble "$STAGE" "$ENGINE_BIN" "$GATEWAY_BIN" "$CLI_BIN" "$APP_DIR/dist" \
    "$PG_PREFIX" "$REPO_ROOT/packages/lucidos-sdk/dist" "$REPO_ROOT/system-knowhow" >/dev/null \
    || die "failed to stage bundle resources into $STAGE"

# ── 5. tauri build ──────────────────────────────────────────────────────────
step "Running cargo tauri build (app + dmg)"
# Inject the resource map at build time (kept out of the committed tauri.conf.json
# so normal `cargo check` / dev builds aren't tied to staged artifacts).
RESOURCES_CONFIG="$(tauri_build_config_json)"
# Trailing `-- --locked` is forwarded to the inner `cargo build` so the release
# bundle is built strictly from the committed Cargo.lock (fail-closed on any
# manifest↔lock drift), matching every other build path.
TAURI_BUILD_ARGS=(tauri build --bundles "app,dmg" --config "$RESOURCES_CONFIG" -- --locked)

# A no-password updater key is still signed with an EMPTY password, not "no
# password env at all". Tauri defaults the password to empty when the var is
# unset, but we set it explicitly so the path stays robust if Tauri ever tightens
# this. (Harmless for the no-password key this repo ships; see context in
# docs/desktop-app.md § Shipping.)
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    # Tauri's macOS codesign skips loose Mach-O Resources (the ~200 relocatable
    # Postgres binaries), so we sign the bundle ourselves further down. We must
    # therefore make Tauri's build NOT macOS-codesign — but WITHOUT suppressing
    # the Tauri UPDATER signature.
    #
    # The `--no-sign` flag does BOTH: it skips macOS codesign AND skips updater
    # signing, so the `.app.tar.gz` was produced with no sibling `.app.tar.gz.sig`
    # (the v0.11.0 upload failure: "no .app.tar.gz.sig produced"). Instead, run the
    # build with APPLE_SIGNING_IDENTITY removed from the SUBPROCESS env only:
    # Tauri sees no identity and skips macOS codesigning, but still signs the
    # updater tarball from TAURI_SIGNING_PRIVATE_KEY, emitting the `.sig`. The
    # outer-shell APPLE_SIGNING_IDENTITY is untouched, so our manual sign below
    # still runs.
    (cd "$APP_DIR" && env -u APPLE_SIGNING_IDENTITY cargo "${TAURI_BUILD_ARGS[@]}")
else
    (cd "$APP_DIR" && cargo "${TAURI_BUILD_ARGS[@]}")
fi

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
    # ${mnt:?} so a hypothetically-empty mountpoint can never turn this into an
    # `rm -rf /<app-name>` against the live filesystem.
    rm -rf "${mnt:?}/$(basename "$app")"
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
begin_step notarize "Refreshing DMG payload, signing the DMG, submitting to the Apple notary service, polling for the verdict, stapling the ticket."
refresh_dmg_payload "$DMG_PATH" "$APP_PATH"

# ── 6. sign DMG + notarize (env-gated) ──────────────────────────────────────
# In --release mode signing + notarization are MANDATORY: a missing credential
# fails loud here rather than silently producing an un-notarized DMG (the v0.10.1
# "notarization silently skipped" fragility). In local mode they stay optional.
# The submit → persist → poll → staple sequence lives in the notarize functions
# defined near the top of this script (they are shared with the resume path); the
# credential rules that used to be documented inline are on notarytool_run.
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    step "Codesigning DMG + notarizing"
    sign_dmg "$DMG_PATH"
    if notarize_credentials_present; then
        notarize_announce_credentials
        if [ "$DEFER_NOTARIZATION" = "1" ]; then
            # Deferred: hand the DMG to Apple, persist the handle, and stop.
            # The bytes still have to be the submitted ones before they are
            # staged — a concurrent rebuild between submit and stage would
            # otherwise publish a DMG that the eventual ticket does not match.
            notarize_submit_and_persist
            assert_dmg_is_the_submitted_bytes "$NOTARIZE_SUBMITTED_SHA"
            DMG_NOTARIZED_STATE="false"
            step "Deferring the notary wait (--defer-notarization)"
            echo "    submission $NOTARIZE_SUBMISSION_ID is in flight; the DMG is signed but NOT stapled."
            echo "    Staging it so the release can publish now. Finish it later with:"
            echo "        scripts/release.sh --attach-notarized $EFFECTIVE_VERSION"
        else
            notarize_submit_and_wait
            # NOTARIZE_SUBMITTED_SHA was recorded by notarize_submit_and_wait before
            # the wait; assert the DMG is still those bytes before stapling.
            staple_notarized_artifacts "$NOTARIZE_SUBMITTED_SHA"
        fi
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
# A DEFERRED run stapled nothing — the submission is still with Apple. Saying
# "Notarized + stapled" here would make the Release Cockpit, the one surface an
# operator checks to see whether a release is deferred, assert the opposite of
# the truth. The step is genuinely finished (this run's notarize work is done),
# so it still succeeds; only the summary tells which outcome it reached.
# --attach-notarized emits the real completion later, through the resume path.
if [ "$DMG_NOTARIZED_STATE" = "false" ]; then
    end_step notarize "Submitted to the Apple notary service and DEFERRED — the DMG is signed but NOT stapled; finish with release.sh --attach-notarized $EFFECTIVE_VERSION."
else
    end_step notarize "Notarized + stapled the DMG and the .app."
fi

# ── 6b–7. stage → drop the resume handle → tarball → upload → report ─────────
# Shared with the resume path so the two can't drift (see
# finalize_release_artifacts near the top of this script).
finalize_release_artifacts
