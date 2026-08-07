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
#   --adopt-app-submission <uuid>
#                              the same for the .app half (see below)
#
# A release makes TWO submissions, in Apple's documented order: the .app first,
# so it can be STAPLED before the DMG is built around it, then the DMG. One
# handle carries both and names which is outstanding in its `stage` field, so a
# resume picks up whichever half died and runs on through the other. See "The
# .app notarization stage" further down, and ADR 0033 for what that costs.
# The handle is dropped once staging succeeds, so a later run can't resume a
# finished release. This exists because the orchestration layer caps background
# tasks at 3600s: a notarization slower than that can never be held in a
# foreground wait, so resumability is the only fix.
#
# ── Notarization deadline (--notarize-deadline) ──────────────────────────────
# Resumability made a lost waiter cheap; it did not make a slow verdict a
# non-event for an UNATTENDED run, because the poll still dies at
# NOTARIZE_POLL_TIMEOUT and a nightly reads that as a failed release. With a
# deadline (a duration, a local wall-clock time, or an absolute epoch, parsed by
# scripts/lib/release_deadline.sh) the poll stops at that instant, the run exits
# with RELEASE_NOTARY_PENDING_EXIT down a "notary pending" path, and NOTHING is
# staged. The build, the codesigns, the signed DMG, the submission and the resume
# handle all survive, so the outstanding verdict is picked up later by the very
# same --resume-notarize above.
#
# It is NOT the deferred mode below wearing a different name. Deferring stages an
# unstapled DMG so the release can PUBLISH now, behind a pending banner; a
# deadline stages nothing and publishes nothing, which is what makes it safe for
# a caller with no human attached. See notarize_deadline_handoff.
#
# ── Deferred DMG (--defer-notarization) ──────────────────────────────────────
# Resumability keeps a slow verdict from costing a rebuild, but the RELEASE still
# waited on it — for 1 to 20 hours, every time. It never had to: notarization
# gates exactly one artifact, the DMG a browser downloads. The headless tarball
# (`curl | sh`) and the Tauri updater (.app.tar.gz + .sig + latest.json) are never
# quarantined: the updater writes the bundle itself and sets no
# com.apple.quarantine, so Gatekeeper performs no assessment on launch. Integrity
# comes from our own minisign key, which IS checked. The payload is Developer ID
# signed as of the repack below, but that signature is NOT the launch mechanism
# and must not be cited as one (F7 in the 2026-08-02 macOS update-path audit: the
# same sentence sat in ADR 0027 and docs/desktop-app.md, and it is the reason
# nobody looked for 19 releases).
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
# Where `cargo tauri build` leaves the .app and the .dmg. A constant, hoisted here
# rather than assigned after the build, because the resume paths reach for it
# BEFORE any build runs (an adopted submission has to find the updater payload the
# submitted DMG is paired with, and `set -u` turns a not-yet-assigned global into
# a crash rather than an empty string).
BUNDLE_DIR="$REPO_ROOT/target/release/bundle"
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

# The notarization deadline (--notarize-deadline). Turns a slow Apple verdict
# from a FAILED run into a clean, resumable pause: the poll stops at a given
# instant, nothing is staged, and the resume handle is left for a later
# --resume-notarize. Pure arithmetic, public-mirror-safe, so source it
# unconditionally like the libs above.
# shellcheck source=scripts/lib/release_deadline.sh
source "$SCRIPT_DIR/lib/release_deadline.sh"

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

# Asset-attach ordering: upload the artifacts, prove they are on the Release,
# and only then upload the latest.json that points at them. One `gh release
# upload` for all four uploaded concurrently, so the smallest file (the manifest)
# finished first and the updater endpoint advertised a payload that was not there
# yet: 10 s on v0.19.0, 8h06m on v0.16.0 (F8). Public-mirror-safe; source
# unconditionally.
# shellcheck source=scripts/lib/release_upload.sh
source "$SCRIPT_DIR/lib/release_upload.sh"

# Which file under target/release/bundle/dmg is THE release DMG. refresh_dmg_payload
# writes two intermediates (.rw.dmg / .zlib.dmg) next to the real artifact, and a
# run killed mid-refresh leaves one behind; both match `*.dmg`, and the
# version-stamp guard cannot tell them apart because they carry the same version
# string. This lib owns the suffixes, so the code that WRITES them and the code
# that EXCLUDES them cannot drift (F4). Public-mirror-safe; source unconditionally.
# shellcheck source=scripts/lib/release_dmg.sh
source "$SCRIPT_DIR/lib/release_dmg.sh"

# Updater-payload repack + re-sign + the Developer ID publish gate. `cargo tauri
# build` packs Lucidos.app.tar.gz from the app BEFORE this script signs it (see
# the codesign section below for why the build runs with the identity stripped
# from its env), so without a repack the updater ships an ad-hoc bundle while the
# DMG ships a Developer ID one. That is the v0.19.0 incident. Depends on
# tauri_signing_key.sh (which it sources itself) for the one signer call site.
# Public-mirror-safe; source it unconditionally.
# shellcheck source=scripts/lib/updater_payload.sh
source "$SCRIPT_DIR/lib/updater_payload.sh"

# Stable dev signing identity (the self-signed cert scripts/dev-codesign-setup.sh
# creates). Used ONLY as the local-build fallback when no APPLE_SIGNING_IDENTITY
# is set: it gives the bundle a certificate-anchored designated requirement, so a
# local rebuild stops destroying the macOS TCC grants the developer has clicked
# through. It can never satisfy a release (see the signing branch below).
# shellcheck source=scripts/lib/codesign.sh
source "$SCRIPT_DIR/lib/codesign.sh"

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
ADOPT_SUBMISSION=""          # --adopt-submission <uuid> (the DMG stage)
ADOPT_APP_SUBMISSION=""      # --adopt-app-submission <uuid> (the .app stage)
NOTARIZE_STATE_FILE=""       # resolved once EFFECTIVE_VERSION is known
NOTARIZE_SUBMISSION_ID=""    # set by notarize_submit
NOTARIZE_STATUS=""           # set by notarize_poll (the terminal Apple verdict)
# The bytes the current notary stage is accountable for, which is what every
# integrity assertion compares against: the app zip at the `app` stage, the DMG
# at the `dmg` stage, and the POST-staple DMG once the ticket is in (the staple
# writes it INTO the image). Same definition as the resume handle's
# artifact_sha256, deliberately. It was NOTARIZE_SUBMITTED_SHA until the staple
# had to be accounted for; "submitted" then stopped being true at exactly the
# moments the value is load-bearing.
NOTARIZE_EXPECTED_SHA=""
# The updater trio this release's submissions are PAIRED with (F3). Captured once
# at the first submit, carried forward to the second, written into the resume
# handle, and re-asserted before anything is stapled or staged. Without them the
# handle pinned the DMG and staging picked up whatever .app.tar.gz was on disk,
# so a concurrent rebuild could put a DMG and an updater payload from two
# different builds into one release.
NOTARIZE_UPDATER_TARBALL=""
NOTARIZE_UPDATER_TARBALL_SHA=""
NOTARIZE_UPDATER_SIG_SHA=""

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
# --notarize-deadline: the absolute epoch past which an outstanding verdict stops
# being worth waiting for. EMPTY on every path that did not pass the flag, and
# every deadline test is written so that empty means "never expires", which is
# what keeps the default behaviour (die on NOTARIZE_POLL_TIMEOUT) untouched.
#
# When it IS set it REPLACES NOTARIZE_POLL_TIMEOUT as the loop's bound rather
# than sitting beside it. Two bounds would mean the shorter one wins, so a
# deadline further out than the 7200s default would still die at 7200s, which is
# precisely the failure the flag exists to remove.
NOTARIZE_DEADLINE=""
NOTARIZE_DEADLINE_EXPIRED=0   # set by notarize_poll when it stops at the deadline

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
  --defer-notarization submit the DMG to Apple, persist the resume handle, and
                       STAGE THE UNSTAPLED DMG instead of waiting for its
                       verdict, so the release can publish now (manifest records
                       notarized:false). Build-grade runs only, never with
                       --release/--release-attach. With --resume-notarize it
                       stages an ALREADY in-flight DMG submission without
                       polling.
                       It defers the DMG's verdict ONLY. A release makes two
                       notary submissions and the .app's comes first, because the
                       DMG is built from the stapled .app, so that one is always
                       waited for.
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
  --adopt-app-submission U
                       the same, for the .app half of the notarization: records
                       UUID U against the .app and the <app>.notarize.zip on
                       disk. Implies --resume-notarize. Only one of the two adopt
                       flags may be given, since only one submission is ever
                       outstanding.
  --notarize-deadline S
                       stop waiting for Apple at S and exit 0 down a "notary
                       pending" path instead of dying: the resume handle, the
                       worktree and the signed artifacts are all left in place,
                       and NOTHING is staged (so no manifest exists for a later
                       publish to promote). S is a duration (90m, 2h, 5400s), a
                       local wall-clock time (06:30, meaning the next time it is
                       06:30), or an absolute epoch (@1785000000). Build-grade
                       runs only, and not with --defer-notarization, which never
                       waits for the DMG's verdict at all. With a deadline set,
                       NOTARIZE_POLL_TIMEOUT no longer bounds the poll.
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
    app_tarball="$(find_updater_tarball)"
    app_sig="$(/usr/bin/find "$BUNDLE_DIR/macos" -maxdepth 1 -name '*.app.tar.gz.sig' 2>/dev/null | head -1 || true)"
    [ -n "$app_tarball" ] || die "no .app.tar.gz produced — is the updater key set (TAURI_SIGNING_PRIVATE_KEY_PATH or TAURI_SIGNING_PRIVATE_KEY)?"
    [ -n "$app_sig" ]     || die "no .app.tar.gz.sig produced — is the updater key set (TAURI_SIGNING_PRIVATE_KEY_PATH or TAURI_SIGNING_PRIVATE_KEY)?"
    source_commit="$(git -C "$REPO_ROOT" rev-parse HEAD)" \
        || die "cannot read git HEAD of $REPO_ROOT for the staging manifest"

    # THE PAIRING GATE (F3), at the chokepoint where the mismatch would become
    # permanent. Everything above this point discovered the updater artifacts by
    # GLOB, which is exactly what let a rebuild slip a newer build's tarball in
    # beside a checksum-pinned DMG: the manifest recorded both, and
    # release_staging_verify found them self-consistent because it only ever
    # compares the staged bytes to the manifest the same run wrote.
    #
    # Two questions, and both have to be asked here rather than earlier: is the
    # tarball on disk the one this submission was paired with (identity), and are
    # all three members still the submitted bytes (integrity)? The window between
    # the staple and this copy is small but it is not zero, and this is the last
    # moment at which refusing is free.
    if [ -n "$NOTARIZE_UPDATER_TARBALL" ] && [ "$app_tarball" != "$NOTARIZE_UPDATER_TARBALL" ]; then
        die "the updater payload about to be staged is not the one this notarization was paired with.
       paired with: $NOTARIZE_UPDATER_TARBALL
       found:       $app_tarball
       Staging this would put a DMG and an updater payload from two different
       builds into one release. Rebuild rather than resuming."
    fi
    # Comparing against the SUBMITTED sha here is what silently unstapled the
    # v0.19.1 DMG: this assertion read our own staple as a rebuild and recovered
    # the pre-staple pin over it. Hence the expected sha rather than a submitted
    # one, which on a stapled release is the post-staple value.
    assert_submitted_set_is_intact "$NOTARIZE_EXPECTED_SHA"

    # THE GATE THAT WOULD HAVE CAUGHT v0.19.1, asserted on the bytes about to be
    # copied rather than inferred from the checksums above. A run that made a
    # notary submission and is not deferred MUST ship a ticket, and nothing else
    # on the path says so out loud: `spctl` resolves the ticket ONLINE, so it
    # answers `accepted / source=Notarized Developer ID` for an unstapled DMG and
    # hid this for a whole release. Same shape as the Developer ID gate below:
    # re-derive the verdict from the bytes at the last moment refusing is free.
    #
    # The condition names the two states where a ticket is genuinely expected. A
    # deferred release stages before anything is stapled (DMG_NOTARIZED_STATE is
    # false, and the manifest says so); a run with no submission id never
    # notarized at all, which release mode already refuses upstream.
    if [ -n "$NOTARIZE_SUBMISSION_ID" ] && [ "$DMG_NOTARIZED_STATE" = "true" ]; then
        step "Verifying the DMG carries its stapled notarization ticket"
        dmg_ticket_is_stapled "$DMG_PATH" \
            || die "refusing to stage $(basename "$DMG_PATH"): it carries NO stapled notarization ticket.
       Notarization completed for submission $NOTARIZE_SUBMISSION_ID, so the
       ticket was stapled and has since been lost, which means something wrote
       over the image after the staple. Gatekeeper would still accept this DMG
       on a machine that can reach Apple, and reject it on one that cannot,
       which is the entire reason the ticket is stapled in the first place.
       Rebuild rather than publishing it."
        echo "    $(basename "$DMG_PATH") validates against its stapled ticket."
    fi

    # THE GATE THAT WOULD HAVE CAUGHT v0.19.0. Staging only ever runs in a
    # release-grade build, where APPLE_SIGNING_IDENTITY is asserted at startup,
    # so a payload that is not Developer ID signed means the repack did not
    # happen (or did not take) and this release would ship an updater that
    # replaces every user's notarized app with an ad-hoc one. Refuse before a
    # single byte reaches the staging dir a later --publish-verified will ship.
    #
    # The refusal has to name the resume handle, because the ONE path that can
    # legitimately reach this with a stale payload is the auto-resume: a run that
    # picks up a handle written by a build predating the repack skips the build
    # AND the repack, so the tarball on disk is that older build's ad-hoc one.
    # "Rebuild" alone would be a dead end there, since the next run auto-resumes
    # into the same wall.
    #
    # Like every other refusal in this function, this die lands with CURRENT_STEP
    # unset (finalize_release_artifacts calls staging after end_step notarize),
    # so it emits no cockpit event. That is consistent with the neighbouring
    # "no .app.tar.gz produced" guards rather than an oversight: the cockpit's
    # step vocabulary has no id for staging, and inventing one it does not render
    # would be worse than the exit code the operator already sees.
    step "Verifying the updater payload is Developer ID signed"
    updater_payload_assert_developer_id "$app_tarball" "$(basename "$app_tarball")" \
        || die "refusing to stage an updater payload that is not Developer ID signed (see above).
       If this run RESUMED a notarization (it says so above) the tarball on disk
       is the one that build produced, and no repack has touched it. Delete the
       resume handle so the next run rebuilds instead of resuming:
           rm ${NOTARIZE_STATE_FILE:-.lucidos/release-state/notarize-<version>.json}"

    # Record WHAT THIS DMG WAS BUILT FROM in content terms, not just commit
    # terms. A later re-fold compares these to decide whether rebuilding could
    # change any shipped byte; without them it must (correctly) rebuild.
    # Non-fatal on failure: a missing fingerprint degrades to today's behaviour
    # (always rebuild), which is safe — losing the whole staging step over it
    # would not be.
    local build_fp recipe_fp
    build_fp="$(release_build_fingerprint_compute "$REPO_ROOT" "$source_commit" 2>/dev/null || true)"
    recipe_fp="$(release_build_recipe_fingerprint_compute "$REPO_ROOT" "$source_commit" 2>/dev/null || true)"

    # Record WHICH PLATFORM this payload is for, read off the app binary that was
    # just signed (F10). This is the only moment the artifact and a machine that
    # can interrogate it are both present: --release-attach deliberately has no
    # .app on disk, and deriving the key there from `uname -m` described the
    # upload host instead. Fatal on failure rather than degrading, because the
    # degraded answer is exactly the silent mislabelling this replaces: an updater
    # whose target key is absent from `platforms` reports "no update", so a wrong
    # key produces no error anywhere, ever.
    local platform_key
    platform_key="$(release_staging_platform_key_for_binary "$APP_PATH/Contents/MacOS/lucidos-app")" \
        || die "could not determine the latest.json platform key for the staged app (see above)."

    rm -rf "$dir"
    mkdir -p "$dir"
    cp "$DMG_PATH" "$dir/"
    cp "$app_tarball" "$dir/"
    cp "$app_sig" "$dir/"
    RELEASE_STAGING_PLATFORM_KEY="$platform_key" \
    RELEASE_STAGING_BUILD_FINGERPRINT="$build_fp" \
    RELEASE_STAGING_RECIPE_FINGERPRINT="$recipe_fp" \
    RELEASE_STAGING_NOTARIZED="$DMG_NOTARIZED_STATE" \
    release_staging_write_manifest "$dir" "$EFFECTIVE_VERSION" "$source_commit" \
        "$(basename "$DMG_PATH")" "$(basename "$app_tarball")" "$(basename "$app_sig")" \
        || die "failed to write the staging manifest in $dir"
    echo "    platform key: $platform_key"
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
    # The same shared discovery the build uses. A staging dir should hold exactly
    # one DMG and no intermediates, so this is belt-and-braces rather than a known
    # hazard, but "publish whichever .dmg find happened to list first" is not a
    # property worth keeping anywhere on the path to a public Release.
    dmg="$(release_dmg_find "$dir" 2>/dev/null || true)"
    app_tarball="$(/usr/bin/find "$dir" -maxdepth 1 -name '*.app.tar.gz' 2>/dev/null | head -1 || true)"
    app_sig="$(/usr/bin/find "$dir" -maxdepth 1 -name '*.app.tar.gz.sig' 2>/dev/null | head -1 || true)"
    [ -n "$dmg" ]         || die "no staged .dmg in $dir"
    [ -n "$app_tarball" ] || die "no staged .app.tar.gz in $dir"
    [ -n "$app_sig" ]     || die "no staged .app.tar.gz.sig in $dir"

    # The same Developer ID gate stage_release_artifacts applies, re-run on the
    # STAGED bytes at the last moment before they become public. Not redundant:
    # --release-attach can be pointed at a staging dir produced by an older
    # build-dmg.sh, or by a run that predates the repack, and nothing else on
    # that path looks inside the tarball. This function is the single chokepoint
    # both upload paths (--release-attach and the one-shot --release) go through,
    # which is what makes "no upload can publish an unsigned payload" a property
    # of the code.
    #
    # Deliberately re-derived from the bytes rather than read back from a
    # manifest field: a recorded verdict would have to be carried forward by
    # every restamp (release.sh's restage_manifest_for_commit), and a restamp
    # that dropped it would launder an unsigned payload into a signed-looking
    # one. That is the trap the `notarized` flag has to keep dodging.
    #
    # Unlike run_release_attach's manifest verify, this one runs INSIDE the
    # `upload` step, so a refusal emits ReleaseStepFailed(upload) and the cockpit
    # goes red rather than staying silent. That is the right signal here and not
    # an oversight: the manifest verify is a precondition checked before the step
    # begins, whereas this is the upload step's own work failing. Being the one
    # chokepoint both upload paths share is worth more than matching the other
    # guard's silence.
    step "Verifying the staged updater payload is Developer ID signed"
    updater_payload_assert_developer_id "$app_tarball" "$(basename "$app_tarball") in $dir" \
        || die "refusing to upload an updater payload that is not Developer ID signed (see above)."

    # THE SAME TICKET GATE stage_release_artifacts applies, re-run on the STAGED
    # bytes at the last moment before they become public, and for the same reason
    # the Developer ID check above is duplicated here: --release-attach can be
    # pointed at a staging dir produced by an older build-dmg.sh, or by the run
    # that shipped the v0.19.1 defect, and nothing else on that path looks at the
    # DMG's ticket. run_release_attach's own pending check reads the manifest's
    # `notarized` FLAG, which is precisely the thing that said `true` over an
    # unstapled DMG, so only re-deriving the verdict from the bytes closes it.
    #
    # The condition is the manifest flag rather than a build-time global, because
    # this function is the chokepoint for BOTH upload paths and the attach one has
    # no build state at all. A deferred publish stages `notarized: false` on
    # purpose and carries the pending banner, so it is skipped here exactly as it
    # is at staging.
    if release_staging_is_notarized "$dir"; then
        step "Verifying the staged DMG carries its notarization ticket"
        dmg_ticket_is_stapled "$dmg" \
            || die "refusing to upload $(basename "$dmg"): the staged DMG carries NO stapled notarization ticket, though its manifest says it is notarized.
       Gatekeeper resolves a missing ticket ONLINE, so this would be accepted on
       a machine that can reach Apple and refused on one that cannot, which is
       the entire reason the ticket is stapled.
       This staging is the one v0.19.1 produced: the manifest's notarized flag
       and the bytes disagree, so re-STAGE it. --attach-notarized cannot fix it
       (it short-circuits on a manifest that already claims to be stapled):
           scripts/release.sh --verify-build $EFFECTIVE_VERSION"
        echo "    $(basename "$dmg") validates against its stapled ticket."
    fi

    # latest.json (the in-app auto-update manifest). The uploaded asset's name is
    # the file's basename, and the updater endpoint resolves
    # …/releases/latest/download/latest.json — so the file must literally be named
    # latest.json. Stage it under that exact name.
    #
    # THE PLATFORM KEY COMES FROM THE MANIFEST (F10), not from `uname -m`. The old
    # `case "$(uname -m)"` here described whichever machine ran the upload, which
    # is not the same question as which architecture the payload is for, and the
    # two diverge on exactly the path that cannot check: --release-attach has no
    # .app on disk to fall back to. release_staging_verify has already refused a
    # manifest that predates the recording, so reaching here means a key exists;
    # the read below is what makes that a hard dependency rather than a hope.
    local platform_key tarball_name download_url pub_date latest_dir latest_json
    platform_key="$(release_staging_platform_key "$dir")" \
        || die "cannot build latest.json for $UPLOAD_TAG (see above)."
    tarball_name="$(basename "$app_tarball")"
    download_url="https://github.com/$REPO_SLUG/releases/download/$UPLOAD_TAG/$tarball_name"
    pub_date="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    latest_dir="$(mktemp -d -t lucidos-latest)"
    latest_json="$latest_dir/latest.json"

    if ! release_upload_write_latest_json "$latest_json" "$EFFECTIVE_VERSION" "$platform_key" \
            "$download_url" "$pub_date" "$app_sig" "${NOTES_FILE:-}"; then
        rm -rf "$latest_dir"
        die "could not generate latest.json for $UPLOAD_TAG (see above)."
    fi

    # THE ORDERING (F8). latest.json goes up LAST, in its own call, after the
    # three artifacts it references are verified present on the Release. Attaching
    # all four in one `gh release upload` uploaded them concurrently, so the
    # smallest file won and the updater endpoint advertised a Lucidos.app.tar.gz
    # GitHub still answered with a 404. The manifest is a separate parameter, not
    # the last artifact, so no argument list can put it back in the first batch.
    if ! release_upload_artifacts_then_manifest "$UPLOAD_TAG" "$REPO_SLUG" "$latest_json" \
            "$dmg" "$app_tarball" "$app_sig"; then
        rm -rf "$latest_dir"
        die "could not attach the release assets to $UPLOAD_TAG (see above)."
    fi
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
# Four outcomes are distinguished, because they need different human responses:
#   • a terminal status                 → return, caller acts on it
#   • an id Apple doesn't recognise     → the handle is stale; a fresh submit is
#                                         required (never silently re-submit)
#   • a transient failure               → retry, up to NOTARIZE_POLL_MAX_FAILURES
#                                         consecutively (a network blip must not
#                                         throw away a 40-minute wait)
#   • --notarize-deadline reached       → set NOTARIZE_DEADLINE_EXPIRED and
#                                         return with NO status. The caller stops
#                                         the run cleanly instead of failing it.
#                                         Only reachable when the flag was given.
notarize_poll() {
    local id="$1"
    local waited=0 fails=0 out status lowered err errfile
    errfile="$(mktemp -t lucidos-notarytool)"
    while :; do
        # Checked at the TOP of the loop as well as before each sleep, so a
        # deadline already in the past when polling starts (a resume picked up
        # after the operator's window closed) stops immediately rather than
        # spending one more round-trip to Apple.
        if release_deadline_expired "$NOTARIZE_DEADLINE"; then
            rm -f "$errfile"
            NOTARIZE_STATUS=""
            NOTARIZE_DEADLINE_EXPIRED=1
            return 0
        fi
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

        # The process ceiling, and the ONE place --notarize-deadline changes an
        # existing behaviour: with a deadline set, the deadline governs and this
        # die is skipped. Without one, this is byte-for-byte what it always did.
        if [ -z "$NOTARIZE_DEADLINE" ] && [ "$waited" -ge "$NOTARIZE_POLL_TIMEOUT" ]; then
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

# notarize_deadline_handoff <submission-id>: stop the run at the deadline,
# cleanly. This is the whole behavioural change --notarize-deadline buys, and it
# is deliberately a full stop rather than a branch the rest of the build flows
# through.
#
# NOTHING IS STAGED. The two options were "stage the unstapled DMG honestly as
# notarized:false" (what --defer-notarization does) and "stage nothing", and this
# path takes the second, for two reasons that are not the deferred case's:
#   1. A release makes two notary submissions and the .app's comes first, so a
#      deadline can expire when no DMG exists at all. There is nothing to stage,
#      and one behaviour for both stages beats two.
#   2. --defer-notarization INTENDS to publish, behind a pending banner. This
#      path must never publish. With no staging dir, --publish-verified refuses
#      outright ("Phase A staging missing") instead of finding a manifest it
#      would happily promote. Fail-closed by absence beats fail-closed by flag.
# Everything expensive survives: the build, the codesigns, the signed DMG, the
# submission with Apple, and the resume handle that ties them together.
#
# The cockpit step SUCCEEDS. A deadline expiry is a pause the operator asked for,
# not a failure, so emitting ReleaseStepFailed would paint the one surface that
# reports release health red for a run that did exactly what it was told. The
# summary carries the outstanding verdict instead. This is the same choice
# --defer-notarization already makes in run_dmg_notarize_stage, and it needs no
# new event type.
notarize_deadline_handoff() {
    local id="$1" version="${EFFECTIVE_VERSION:-<version>}"
    end_step notarize "Reached the --notarize-deadline with Apple's verdict on submission $id still outstanding. Nothing was staged and nothing failed; finish with release.sh --resume-notarize $version."
    cat <<EOF

────────────────────────────────────────────────────────────────────────────
NOTARY PENDING: the deadline passed before Apple answered.
────────────────────────────────────────────────────────────────────────────
  Submission: $id  (still in flight with Apple)
  Deadline:   $(release_deadline_format "$NOTARIZE_DEADLINE")
  Handle:     $NOTARIZE_STATE_FILE
  Worktree:   $REPO_ROOT

  This is NOT a failure. The build, the codesigns and the signed artifacts are
  all on disk, and the submission is still with Apple, so finishing costs a poll
  rather than a rebuild.

  NOTHING WAS STAGED, on purpose: with no staging dir there is no manifest for
  --publish-verified to promote, so this run cannot have left an unstapled DMG
  anywhere it could be published from.

  Pick it back up once the verdict lands:
      scripts/release.sh --resume-notarize $version
EOF
    exit "$RELEASE_NOTARY_PENDING_EXIT"
}

# notarize_await_verdict <submission-id> — poll to a terminal status and act:
# Accepted continues to stapling; anything else prints the notary log and fails
# loud WITHOUT stapling or staging (a rejected build must never reach a staging
# manifest, which is what --publish-verified would go on to ship).
#
# A --notarize-deadline expiry is handled HERE rather than at each of the three
# call sites, because "the deadline passed" has the same answer at every one of
# them and this is the single chokepoint they all pass through. It is also the
# last point before anything is stapled or staged, which is what makes the
# no-staging promise a property of the code rather than of three call sites
# remembering.
notarize_await_verdict() {
    local id="$1"
    notarize_poll "$id"
    if [ "$NOTARIZE_DEADLINE_EXPIRED" = "1" ]; then
        notarize_deadline_handoff "$id"
    fi
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

# ── The submitted set: pin it, then prove it is still intact ─────────────────
#
# THE 2026-07-28 ORPHANED-POLLER BUG. build-dmg.sh writes the DMG to a FIXED path
# (target/release/bundle/dmg/Lucidos_<version>_aarch64.dmg), so a rebuild
# overwrites the exact file an in-flight submission was for. That day three
# pollers were alive at once and two were waiting on submissions whose bytes no
# longer existed. Had those verdicts returned, each would have stapled a ticket
# issued for one set of bytes onto a different set.
#
# THE F3 EXTENSION. The guard that fixed that covered the DMG and nothing else,
# and the release does not ship a DMG alone: it ships the DMG, `Lucidos.app.tar.gz`
# and its `.sig`, which must all come from ONE build. Worse than a passive gap,
# the recovery below actively creates the mismatch if it is applied per-artifact:
# it restores the DMG from its pin after a concurrent rebuild, which is exactly
# the state in which the tarball on disk belongs to the newer build. So the set is
# pinned together and asserted together.

# notarize_pin_dir <sha256>: the content-addressed directory a pinned artifact
# lives in. Addressed by CONTENT rather than by submission, so two concurrent
# builds of the same version cannot collide, which is the 2026-07-28 shape.
notarize_pin_dir() {
    printf '%s/.lucidos/notarize-submissions/%s/%s' \
        "$REPO_ROOT" "${EFFECTIVE_VERSION:-unversioned}" "${1:0:12}"
}

# notarize_pin_artifact <path> <sha256>: keep an immutable copy of <path> under
# its content address, so a later run can RECOVER those bytes instead of losing a
# completed notarization. Usually the bytes Apple scanned, and after the staple
# the bytes that carry the ticket (see notarize_record_stapled_dmg), which is why
# this is worded by content address rather than by "submitted".
#
# WHY A CLONE AND NOT A HARDLINK. A hardlink is the obvious zero-cost pin and it
# is WRONG here: it is a second name for the SAME inode, so anything that writes
# the file IN PLACE (`codesign` rewriting a signature, a truncating `>`) is seen
# through both names and silently corrupts the pin. The test suite proves this: an
# in-place write mutates a hardlinked pin while leaving a cloned one intact.
# `cp -c` requests an APFS clonefile, which is copy-on-write, so it costs no disk
# until one side diverges and an in-place write to the original allocates new
# blocks instead of touching the pinned copy. On a non-APFS volume the -c fails
# and the plain copy takes over: correct, just not free.
#
# Best-effort by design. If the pin cannot be created the build proceeds unpinned:
# the checksum assertion is the correctness guarantee (it refuses rather than
# mis-staples), and the pin is only what additionally lets a poller recover.
notarize_pin_artifact() {
    local path="$1" sha="$2" pin_dir pin
    [ -n "$sha" ] || return 0
    [ -f "$path" ] || return 0
    pin_dir="$(notarize_pin_dir "$sha")"
    if ! mkdir -p "$pin_dir" 2>/dev/null; then
        echo "    NOTE: could not create $pin_dir, so $(basename "$path") is unpinned."
        return 0
    fi
    pin="$pin_dir/$(basename "$path")"
    if [ -f "$pin" ]; then
        echo "    pinned (already): $pin"
        return 0
    fi
    if cp -c "$path" "$pin" 2>/dev/null || cp "$path" "$pin" 2>/dev/null; then
        echo "    pinned: $pin"
    else
        echo "    NOTE: could not pin $path, so it is unpinned."
        rm -f "$pin"
    fi
}

# notarize_find_pin <sha256>: print a pinned file whose bytes hash to <sha256>, or
# nothing. Located by content address alone, so a FRESH process (the orphaned
# poller, which knows nothing about what the dead one pinned) finds it just as
# well as the process that created it.
notarize_find_pin() {
    local sha="$1" candidate
    [ -n "$sha" ] || return 0
    for candidate in "$(notarize_pin_dir "$sha")"/*; do
        [ -f "$candidate" ] || continue
        if [ "$(release_staging_sha256 "$candidate" 2>/dev/null || true)" = "$sha" ]; then
            printf '%s' "$candidate"
            return 0
        fi
    done
    return 0
}

# notarize_pin_submitted_set <artifact> <artifact-sha256>: pin the artifact about
# to be handed to Apple TOGETHER WITH the updater payload it is paired with, so a
# recovery can restore a whole build rather than half of one.
notarize_pin_submitted_set() {
    notarize_pin_artifact "$1" "$2"
    if [ -n "$NOTARIZE_UPDATER_TARBALL" ]; then
        notarize_pin_artifact "$NOTARIZE_UPDATER_TARBALL" "$NOTARIZE_UPDATER_TARBALL_SHA"
        notarize_pin_artifact "$NOTARIZE_UPDATER_TARBALL.sig" "$NOTARIZE_UPDATER_SIG_SHA"
    fi
}

# assert_submitted_artifacts_are_intact <label> <path> <sha256> [<label> <path> <sha256>…]
#
# Refuse to staple or stage unless EVERY member of the submitted set is still the
# bytes that were submitted, restoring from the pins where it can.
#
# DECIDE FIRST, THEN ACT, and that ordering is the whole point. Three separate
# `cp`s cannot be atomic on a filesystem, but nothing needs to be: what must never
# happen is restoring SOME members and proceeding. The loop below therefore only
# records what it would restore; it copies nothing until every member is known to
# be either intact or recoverable, and on any unrecoverable member it dies having
# touched nothing. The previous version copied the DMG the instant it noticed
# drift, which is precisely how a release ends up holding half a build.
#
# A member with an empty expected sha is skipped: a local build with no updater
# key has no payload to compare, and the release-grade refusal for a missing one
# belongs to stage_release_artifacts.
# SC2317: ShellCheck can see that `die` ends in `exit`, so it calls the `return 1`
# after each die unreachable. They are deliberate, and the comment inside the
# function says why: the refusal must be this function's own behaviour, not a
# consequence of a helper's exit semantics.
# shellcheck disable=SC2317
assert_submitted_artifacts_are_intact() {
    local label path sha actual pin detail i
    local -a restore_from=() restore_to=() restore_label=() problems=()

    while [ "$#" -ge 3 ]; do
        label="$1"; path="$2"; sha="$3"; shift 3
        # No recorded checksum means there is nothing to assert: a local build
        # with no updater key has no payload to compare against.
        [ -n "$sha" ] || continue
        # A recorded checksum with NO path is an inconsistent caller, and it must
        # refuse rather than skip. Skipping would silently drop a member from the
        # set, which is the exact class of hole this function exists to close.
        if [ -z "$path" ]; then
            problems+=("$label: submitted $sha, but no path was given to check it against")
            continue
        fi
        if [ -f "$path" ]; then
            actual="$(release_staging_sha256 "$path" 2>/dev/null || true)"
            [ "$actual" = "$sha" ] && continue
            [ -n "$actual" ] || actual="(unreadable)"
        else
            actual="(missing)"
        fi
        pin="$(notarize_find_pin "$sha")"
        if [ -n "$pin" ]; then
            restore_from+=("$pin")
            restore_to+=("$path")
            restore_label+=("$label")
        else
            problems+=("$label: submitted $sha, on disk $actual, and no pinned copy survives")
        fi
    done
    # Every refusal below `return`s as well as calling `die`. `die` exits in
    # build-dmg.sh, so the return is unreachable there, and that is the point:
    # whether this function refuses must be a property of THIS function rather
    # than of a helper's exit semantics. Without it, a `die` that ever became
    # non-exiting would fall through to the restore loop and produce exactly the
    # half-restored tree the refusal exists to prevent.
    if [ "$#" -ne 0 ]; then
        die "assert_submitted_artifacts_are_intact takes <label> <path> <sha256> triples; got $# trailing argument(s)"
        return 1
    fi

    if [ "${#problems[@]}" -gt 0 ]; then
        detail="$(printf '       %s\n' "${problems[@]}")"
        die "REFUSING TO STAPLE OR STAGE: the set Apple scanned is no longer on disk.
$detail
       Another build replaced these while the submission was in flight. Restoring
       only the members that CAN be recovered is what produces the failure this
       exists to prevent: a DMG from one build staged beside an updater payload
       from another, self-consistent in the manifest and wrong on disk.
       NOTHING has been restored. Rebuild, or resume from the tree that built it."
        return 1
    fi

    i=0
    while [ "$i" -lt "${#restore_to[@]}" ]; do
        echo "    NOTE: ${restore_label[$i]} no longer holds the submitted bytes (a rebuild replaced it)."
        echo "          Recovering them from the pin: ${restore_from[$i]}"
        if ! cp -f "${restore_from[$i]}" "${restore_to[$i]}"; then
            die "could not restore ${restore_label[$i]} from its pin at ${restore_from[$i]}"
            return 1
        fi
        i=$((i + 1))
    done
}

# assert_submitted_set_is_intact <expected-artifact-sha256>: the same assertion,
# over the set this run actually submitted: the artifact Apple scanned plus the
# updater payload and signature it was paired with.
assert_submitted_set_is_intact() {
    local expected="${1:-}" sig=""
    if [ -n "$NOTARIZE_UPDATER_TARBALL" ]; then
        sig="$NOTARIZE_UPDATER_TARBALL.sig"
    fi
    assert_submitted_artifacts_are_intact \
        "$(basename "$DMG_PATH")" "$DMG_PATH"                 "$expected" \
        "the updater payload"     "$NOTARIZE_UPDATER_TARBALL" "$NOTARIZE_UPDATER_TARBALL_SHA" \
        "the updater signature"   "$sig"                      "$NOTARIZE_UPDATER_SIG_SHA"
}

# dmg_ticket_is_stapled <path>: zero when <path> carries a valid stapled ticket,
# non-zero when stapler says it does not. The three gates that ask this question
# share it so they cannot drift, and so none of them has to repeat the reason it
# is not a bare `xcrun stapler validate`.
#
# A MISSING TICKET AND A MISSING TOOLCHAIN ARE OPPOSITE ANSWERS. `stapler`
# reports "does not have a ticket stapled to it" with exit **65**, and that is
# the only non-zero exit that is a verdict about the DMG. Exit 127 (no `xcrun`)
# or an unselected developer dir (`tool 'stapler' requires Xcode`) means the
# question was never asked, and reporting that as "no ticket" sends the operator
# into a 40-minute rebuild and re-notarization for what is an
# `xcode-select --install`. So anything other than 0 or 65 dies here, quoting
# what stapler actually said.
#
# SC2317: the `return 1` after `die` is deliberate, for the reason spelled out in
# assert_submitted_artifacts_are_intact: refusing must be this function's own
# behaviour rather than a consequence of a helper's exit semantics. It matters
# more here than anywhere, because this function's callers branch on its status.
# shellcheck disable=SC2317
dmg_ticket_is_stapled() {
    local path="$1" out rc=0
    out="$(xcrun stapler validate "$path" 2>&1)" || rc=$?
    case "$rc" in
        0)  return 0 ;;
        65) return 1 ;;
    esac
    die "could not tell whether $(basename "$path") carries a notarization ticket: \`xcrun stapler validate\` exited $rc, which is neither 0 (stapled) nor 65 (not stapled).
$out
       That is a toolchain failure, not a verdict about the DMG. Check that the
       Command Line Tools are installed and selected: xcode-select -p"
    return 1
}

# notarize_carry_staple_into_handle <post-staple-sha256>: move the resume
# handle's record of the DMG forward with the other two.
#
# The expectation lives in THREE places, and leaving one behind is the same
# asymmetry that produced the bug this whole section is about. The global drives
# this process, the pin is what a recovery reads, and the handle is what a LATER
# process reads. `release_notarize_resumable` re-hashes `artifact_path` against
# `artifact_sha256` and refuses on any mismatch, so a handle left describing the
# pre-staple bytes makes the run unresumable the moment the DMG is stapled. That
# window is small but it is exactly the deferred release's: `--attach-notarized`
# staples an ALREADY-PUBLISHED DMG and then stages, and a staging failure after
# the staple would strand it, unstaplable without a full rebuild, which is the
# one outcome ADR 0027 says the handle exists to prevent.
#
# The field keeps its meaning: `artifact_sha256` is the bytes this submission is
# accountable for, which are the submitted bytes right up until we add our own
# ticket to them. Only the DMG stage can reach here, so the `app` stage's record
# of the submitted zip is never touched.
#
# Best-effort, like the pin, and for the same reason: the staple has already
# succeeded and the assertion (not the handle) is the correctness guarantee, so
# failing a release over bookkeeping would be the wrong trade. Say so, though, so
# the operator knows a crash before staging will need the rebuild.
notarize_carry_staple_into_handle() {
    local sha="$1" commit submitted_at
    [ -n "$NOTARIZE_STATE_FILE" ] && [ -f "$NOTARIZE_STATE_FILE" ] || return 0
    [ -n "$NOTARIZE_SUBMISSION_ID" ] || return 0
    # A FAILED READ MUST NOT BECOME A WRITTEN EMPTY FIELD. These two are carried
    # over verbatim rather than regenerated, so if either read fails (corrupt
    # JSON, a transient python3 failure) the rewrite would replace a good handle
    # with one whose source_commit is "", and the resume gate would then refuse
    # with "the tree moved since the artifact was built" against a tree that
    # never moved: the exact stranding this function exists to prevent, wearing
    # the wrong diagnosis. Leave the handle alone and say so instead.
    if ! commit="$(release_notarize_field "$NOTARIZE_STATE_FILE" source_commit 2>/dev/null)" \
       || ! submitted_at="$(release_notarize_field "$NOTARIZE_STATE_FILE" submitted_at 2>/dev/null)"; then
        echo "    NOTE: could not read $NOTARIZE_STATE_FILE back, so it still describes the pre-staple bytes; a failure before staging will need a rebuild rather than a resume."
        return 0
    fi
    RELEASE_NOTARIZE_UPDATER_TARBALL="$NOTARIZE_UPDATER_TARBALL" \
    RELEASE_NOTARIZE_UPDATER_TARBALL_SHA256="$NOTARIZE_UPDATER_TARBALL_SHA" \
    RELEASE_NOTARIZE_UPDATER_SIG_SHA256="$NOTARIZE_UPDATER_SIG_SHA" \
    release_notarize_write_state "$NOTARIZE_STATE_FILE" "$RELEASE_NOTARIZE_STAGE_DMG" \
        "$NOTARIZE_SUBMISSION_ID" "$DMG_PATH" "$sha" "$EFFECTIVE_VERSION" \
        "$commit" "$submitted_at" \
        || echo "    NOTE: could not update $NOTARIZE_STATE_FILE to the stapled bytes; a failure before staging will need a rebuild rather than a resume."
}

# notarize_record_stapled_dmg: the staple REWROTE the DMG, so make the stapled
# bytes what every later assertion expects, pin them, and carry them into the
# handle.
#
# THE v0.19.1 PHASE A DEFECT. `xcrun stapler staple` writes the ticket INTO the
# disk image, so the file the guard protects necessarily changes at the one
# moment the guard does not expect it to, and nothing above could tell that
# INTENDED mutation from the concurrent rebuild the guard exists to catch. The
# staple succeeded; stage_release_artifacts then ran its own
# assert_submitted_set_is_intact, found the mismatch, located the PRE-STAPLE pin
# and copied it back over the stapled DMG. The staple was silently undone, the
# manifest recorded the unstapled sha, release_staging_verify found the pair
# self-consistent, and --publish-verified would have shipped a DMG carrying no
# ticket. `spctl` said `accepted / source=Notarized Developer ID` throughout,
# because that is an ONLINE lookup, the exact dependency stapling exists to
# remove; that is why Gatekeeper acceptance hid it and the rc DMG-verify leg
# (`stapler validate`, exit 65) is what caught it.
#
# EVERY HALF MOVES FORWARD, and each is load-bearing. Re-recording keeps a
# rebuild after this point DETECTED: the expected sha is the one now on disk, so
# a replacement still mismatches with the same force as before. Re-pinning keeps
# it RECOVERABLE, and by the right bytes: the pre-staple pin can only ever
# restore a DMG with no ticket, which is the failure above wearing a different
# hat. Pins are content-addressed, so the stapled copy sits BESIDE the submitted
# one rather than replacing it, and finalize_release_artifacts drops the whole
# version's pin dir once staging holds a copy. The handle is the third
# (notarize_carry_staple_into_handle, above).
#
# Only the DMG needs this. staple_notarized_artifacts also staples the standalone
# .app, and no later assertion reads that bundle's bytes: the app stage asserts
# its submitted ZIP (which stapling the bundle does not touch) and its CDHash
# (which the ticket does not change, since it lands in Contents/CodeResources,
# outside the sealed set, which is why that stage's codesign --verify passes).
#
# SC2317: `die` ends in `exit`, so ShellCheck reads the `return 1` as
# unreachable. It is deliberate, for the reason spelled out in
# assert_submitted_artifacts_are_intact: refusing must be this function's own
# behaviour rather than a consequence of a helper's exit semantics.
# shellcheck disable=SC2317
notarize_record_stapled_dmg() {
    local sha again
    sha="$(release_staging_sha256 "$DMG_PATH" 2>/dev/null || true)"
    if [ -z "$sha" ]; then
        die "could not re-hash $DMG_PATH after stapling it. Every later integrity check compares against that value, and the staple has just changed the bytes it describes, so proceeding would either restore the unstapled copy over the ticket or refuse to stage at all."
        return 1
    fi
    # PROVE THE TICKET BEFORE ADOPTING THE BYTES, then prove the bytes did not
    # move while we proved it. Whatever this function records becomes what the
    # release stages, so the one thing it must never do is bless a file it has
    # not checked. The window between the staple returning and the hash above is
    # small, but the DMG lives at a FIXED path and a concurrent build overwrites
    # exactly that path, which is the 2026-07-28 shape; without these two checks
    # a rebuild landing in that window would be adopted as "the stapled bytes"
    # and pinned as the recovery copy, and the release would ship a DMG that was
    # never submitted to Apple. This is also the check the .app stage has always
    # made after its own staple; the DMG half never did, which is why the missing
    # ticket had to travel all the way to a CI runner to be noticed.
    #
    # It proves A ticket, not OUR ticket, and the gap is accepted: another build
    # would have to finish building, submitting, waiting out Apple and stapling
    # inside a window this function closes in milliseconds. The pairing gate in
    # stage_release_artifacts is the backstop if it ever happened, since that
    # build's tarball would not be the one this submission is paired with.
    dmg_ticket_is_stapled "$DMG_PATH" || {
        die "stapler reported success for $DMG_PATH but the bytes now at that path carry no valid ticket. Another build almost certainly replaced the image between the staple and this check. Refusing to record them: they would be staged and published as the notarized DMG."
        return 1
    }
    again="$(release_staging_sha256 "$DMG_PATH" 2>/dev/null || true)"
    if [ "$again" != "$sha" ]; then
        die "$DMG_PATH changed while its staple was being verified (was $sha, now ${again:-(unreadable)}). Another build is writing to this path. Refusing to record either version."
        return 1
    fi
    if [ "$sha" = "$NOTARIZE_EXPECTED_SHA" ]; then
        # staple_idempotent's already-carries-a-ticket branch, which is normal on
        # a resume. Nothing moved, so say so rather than claiming a rewrite.
        echo "    $(basename "$DMG_PATH") already held its ticket; the expected bytes are unchanged."
    else
        echo "    the staple rewrote $(basename "$DMG_PATH"); later checks now expect $sha"
    fi
    NOTARIZE_EXPECTED_SHA="$sha"
    notarize_pin_artifact "$DMG_PATH" "$sha"
    notarize_carry_staple_into_handle "$sha"
    # A pin that does not hold the bytes it is addressed by is worse than no pin:
    # notarize_find_pin verifies content, so such a pin is simply never found and
    # the recoverability this re-pin exists for is silently gone. Pinning stays
    # best-effort (the assertion, not the pin, is the correctness guarantee), so
    # say so rather than fail the release over it.
    if [ -z "$(notarize_find_pin "$sha")" ]; then
        echo "    NOTE: the stapled bytes are unpinned, so a rebuild after this point will refuse rather than recover."
    fi
}

# staple_notarized_artifacts [<expected-dmg-sha256>] — staple the DMG and (when
# present) the .app. When the caller knows the sha256 that was submitted it MUST
# pass it: stapling different bytes than Apple scanned is the failure mode this
# guards (see assert_submitted_artifacts_are_intact).
#
# THE CHOKEPOINT FOR THE STAPLE ITSELF. Both paths that staple a DMG come through
# here, the fresh build and the resume behind --attach-notarized, so the expected
# bytes move forward in exactly one place and the two cannot drift.
# --defer-notarization never reaches this function at all: it stages the
# unstapled DMG with notarized:false and keeps comparing against the submitted
# sha, which is correct, because its submission is still in flight.
staple_notarized_artifacts() {
    assert_submitted_set_is_intact "${1:-}"
    step "Stapling the notarization ticket"
    staple_idempotent "$DMG_PATH"
    # Immediately, before anything else can observe the stale expectation.
    notarize_record_stapled_dmg
    # The standalone .app normally already carries a ticket by now: the app stage
    # stapled it before the DMG was built around it, so this reports "already
    # carries a valid ticket" and changes nothing. It is kept rather than dropped
    # because the app stage is skipped when there are no Apple credentials, and
    # an adopted DMG submission can be resumed on a tree whose app never went
    # through it. Stapling the shipped copy is the app stage's job (F5); this only
    # keeps the standalone bundle consistent with it.
    if [ -n "$APP_PATH" ] && [ -e "$APP_PATH" ]; then
        staple_idempotent "$APP_PATH"
    else
        echo "    (no .app on disk to staple; the DMG carries its own ticket)"
    fi
}

# find_updater_tarball: the `.app.tar.gz` this build produced, or empty. One
# definition, so the repack, the pairing capture, staging and the closing report
# all mean the same file.
find_updater_tarball() {
    /usr/bin/find "$BUNDLE_DIR/macos" -maxdepth 1 -name '*.app.tar.gz' 2>/dev/null | head -1 || true
}

# notarize_capture_updater_pairing: record WHICH updater payload this build's
# submission is paired with, into the three NOTARIZE_UPDATER_* globals the handle
# writer reads.
#
# THE F3 PAIRING. The handle used to pin the DMG's bytes and say nothing about
# the `.app.tar.gz` + `.sig`, which staging then picked up by glob from whatever
# was on disk at that moment. Nothing tied them to the build that produced the
# pinned DMG, and the recovery branch of the staple-time assertion made it worse
# rather than passive: it RESTORES the DMG from its pin after a concurrent
# rebuild overwrote it, which is precisely the state in which the tarball beside
# it belongs to the newer build. The staging manifest then recorded both,
# release_staging_verify found them self-consistent because it only ever checks
# internal consistency, and the release shipped a DMG and an updater payload from
# two different builds.
#
# Captured ONCE, at the first submission of the release, and carried forward to
# the second (see notarize_carry_updater_pairing_forward). Re-capturing at the
# second submit would let a tarball replaced during the first wait be adopted as
# the pairing: self-consistent again, and wrong again.
#
# An empty capture is legitimate and distinct from an absent one: a build with no
# updater key produces no tarball at all, the handle records the keys empty, and
# the pairing checks become vacuous. The release-grade refusal for a missing
# payload belongs to stage_release_artifacts, which explains it in actionable
# terms.
notarize_capture_updater_pairing() {
    NOTARIZE_UPDATER_TARBALL="$(find_updater_tarball)"
    NOTARIZE_UPDATER_TARBALL_SHA=""
    NOTARIZE_UPDATER_SIG_SHA=""
    [ -n "$NOTARIZE_UPDATER_TARBALL" ] || return 0

    NOTARIZE_UPDATER_TARBALL_SHA="$(release_staging_sha256 "$NOTARIZE_UPDATER_TARBALL")" \
        || die "could not hash $NOTARIZE_UPDATER_TARBALL to pair it with this submission"
    if [ -f "$NOTARIZE_UPDATER_TARBALL.sig" ]; then
        NOTARIZE_UPDATER_SIG_SHA="$(release_staging_sha256 "$NOTARIZE_UPDATER_TARBALL.sig")" \
            || die "could not hash $NOTARIZE_UPDATER_TARBALL.sig to pair it with this submission"
    fi
}

# notarize_carry_updater_pairing_forward: reload the pairing the FIRST submission
# of this release recorded, so the second submission records the identical values
# instead of re-reading disk. See the note above on why re-capturing would defeat
# the purpose. Falls back to a fresh capture when there was no first submission.
notarize_carry_updater_pairing_forward() {
    if [ -n "$NOTARIZE_STATE_FILE" ] && [ -f "$NOTARIZE_STATE_FILE" ] \
       && release_notarize_has_fields "$NOTARIZE_STATE_FILE" updater_tarball_path; then
        NOTARIZE_UPDATER_TARBALL="$(release_notarize_field "$NOTARIZE_STATE_FILE" updater_tarball_path)"
        NOTARIZE_UPDATER_TARBALL_SHA="$(release_notarize_field "$NOTARIZE_STATE_FILE" updater_tarball_sha256)"
        NOTARIZE_UPDATER_SIG_SHA="$(release_notarize_field "$NOTARIZE_STATE_FILE" updater_sig_sha256)"
        return 0
    fi
    notarize_capture_updater_pairing
}

# notarize_submit_and_persist — submit and durably record the handle, stopping
# short of the wait. Split out of notarize_submit_and_wait because a DEFERRED
# release needs exactly this half: the submission is with Apple and recoverable
# from disk, and the build is free to stage and publish without the verdict.
notarize_submit_and_persist() {
    local sha commit submitted_at
    sha="$(release_staging_sha256 "$DMG_PATH")" \
        || die "could not hash $DMG_PATH for the notarize resume handle"
    NOTARIZE_EXPECTED_SHA="$sha"
    commit="$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || true)"
    submitted_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    # The DMG is the SECOND submission of a release, so the pairing comes from the
    # app stage's handle rather than from a fresh read of disk.
    notarize_carry_updater_pairing_forward

    # PIN THE SUBMITTED SET before handing the DMG to Apple. The DMG lives at a
    # FIXED path that the next build overwrites, so the file the notary is
    # scanning can silently become different bytes mid-flight (2026-07-28: three
    # concurrent pollers, two of them orphaned this way). The updater payload and
    # its signature live at fixed paths too and are overwritten by the same
    # rebuild, so pinning only the DMG is what let a recovery restore half a
    # build (F3).
    notarize_pin_submitted_set "$DMG_PATH" "$sha"

    step "Submitting $(basename "$DMG_PATH") to the Apple notary service"
    notarize_submit "$DMG_PATH"

    # Persist BEFORE the first poll. Everything above this line is expensive and
    # already on disk; from here on, losing this process costs a poll rather than a
    # full rebuild (the 2026-07-28 incident, where the id died with the waiter).
    RELEASE_NOTARIZE_UPDATER_TARBALL="$NOTARIZE_UPDATER_TARBALL" \
    RELEASE_NOTARIZE_UPDATER_TARBALL_SHA256="$NOTARIZE_UPDATER_TARBALL_SHA" \
    RELEASE_NOTARIZE_UPDATER_SIG_SHA256="$NOTARIZE_UPDATER_SIG_SHA" \
    release_notarize_write_state "$NOTARIZE_STATE_FILE" "$RELEASE_NOTARIZE_STAGE_DMG" \
        "$NOTARIZE_SUBMISSION_ID" "$DMG_PATH" "$sha" "$EFFECTIVE_VERSION" \
        "$commit" "$submitted_at" \
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
    local head_commit="$1" dmg_dir dmg sha
    dmg_dir="$REPO_ROOT/target/release/bundle/dmg"
    # release_dmg_find excludes refresh_dmg_payload's intermediates and refuses an
    # ambiguous directory: a run killed mid-refresh can leave a .rw.dmg /
    # .zlib.dmg behind, and adopting one of those would record a checksum for
    # bytes Apple never saw. Its reason lands on stderr just above this die.
    dmg="$(release_dmg_find "$dmg_dir")" \
        || die "--adopt-submission could not identify the built .dmg that was submitted (the reason is above). Adoption needs exactly one signed DMG still on disk under $dmg_dir."
    case "$(basename "$dmg")" in
        *"_${EFFECTIVE_VERSION}_"*) ;;
        *) die "--adopt-submission: the on-disk DMG '$(basename "$dmg")' does not carry version '$EFFECTIVE_VERSION' — refusing to adopt a submission for a different build." ;;
    esac
    sha="$(release_staging_sha256 "$dmg")" || die "could not hash $dmg"
    step "Adopting in-flight submission $ADOPT_SUBMISSION for $(basename "$dmg")"
    # Adoption is the one path with no earlier submission to inherit the pairing
    # from, so it captures the trio from disk. That is the best available claim:
    # the DMG on disk is being taken as the submitted one, and the tarball beside
    # it is the same build's on exactly the same evidence.
    notarize_capture_updater_pairing
    RELEASE_NOTARIZE_UPDATER_TARBALL="$NOTARIZE_UPDATER_TARBALL" \
    RELEASE_NOTARIZE_UPDATER_TARBALL_SHA256="$NOTARIZE_UPDATER_TARBALL_SHA" \
    RELEASE_NOTARIZE_UPDATER_SIG_SHA256="$NOTARIZE_UPDATER_SIG_SHA" \
    release_notarize_write_state "$NOTARIZE_STATE_FILE" "$RELEASE_NOTARIZE_STAGE_DMG" \
        "$ADOPT_SUBMISSION" "$dmg" "$sha" "$EFFECTIVE_VERSION" \
        "$head_commit" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        || die "could not write the notarize resume handle to $NOTARIZE_STATE_FILE"
    echo "    wrote $NOTARIZE_STATE_FILE (submitted_at records the adoption time)"
}

# sign_dmg <dmg>: Developer ID sign the finished disk image. Up here with the
# other notarize helpers rather than in the build section, because the resume
# path reaches it before any build runs.
sign_dmg() {
    local dmg="$1"
    codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$dmg"
    codesign --verify --strict --verbose=2 "$dmg"
}

# refresh_dmg_payload_cleanup <rw> <out> <mnt>: unwind whatever this refresh has
# created so far. Called on every failure branch below and on the success path.
#
# It exists because the two intermediates are the raw material of F4: a run that
# dies between the convert and the trailing `rm -f` leaves a `.rw.dmg` sitting in
# the bundler's output dir permanently, where the next build's discovery has to
# know to skip it. The exclusion in release_dmg.sh is the guard; not creating the
# litter in the first place is the fix. A left-behind MOUNT is worse still: the
# volume stays attached after the build exits, and the next `hdiutil attach` of
# the same image fails.
#
# Explicit calls rather than a `trap … RETURN`: under `set -e` a failing command
# does not return from the function, it exits the shell through the ERR trap, and
# a RETURN trap never fires on that path.
refresh_dmg_payload_cleanup() {
    local rw="$1" out="$2" mnt="$3"
    if [ -n "$mnt" ] && [ -d "$mnt" ]; then
        hdiutil detach "$mnt" -force >/dev/null 2>&1 || true
        rmdir "$mnt" 2>/dev/null || true
    fi
    rm -f "$rw" "$out"
}

refresh_dmg_payload() {
    local dmg="$1"
    local app="$2"
    local rw out mnt
    rw="$(release_dmg_rw_path "$dmg")"
    out="$(release_dmg_zlib_path "$dmg")"
    mnt="$(mktemp -d)"
    # Clear BOTH intermediates up front, not just the read-write one: a previous
    # run killed during the recompress leaves the .zlib.dmg behind, and hdiutil
    # refuses to convert onto an existing path.
    rm -f "$rw" "$out"
    hdiutil convert "$dmg" -format UDRW -o "$rw" >/dev/null \
        || { refresh_dmg_payload_cleanup "$rw" "$out" "$mnt"; die "hdiutil could not convert $(basename "$dmg") to a read-write image"; }
    hdiutil attach "$rw" -nobrowse -noautoopen -mountpoint "$mnt" >/dev/null \
        || { refresh_dmg_payload_cleanup "$rw" "$out" "$mnt"; die "hdiutil could not mount $(basename "$rw")"; }
    # ${mnt:?} so a hypothetically-empty mountpoint can never turn this into an
    # `rm -rf /<app-name>` against the live filesystem.
    rm -rf "${mnt:?}/$(basename "$app")"
    ditto "$app" "$mnt/$(basename "$app")" \
        || { refresh_dmg_payload_cleanup "$rw" "$out" "$mnt"; die "could not copy $(basename "$app") into the mounted DMG"; }
    [ -f "$mnt/.VolumeIcon.icns" ] && chflags hidden "$mnt/.VolumeIcon.icns"
    hdiutil detach "$mnt" -force >/dev/null
    rmdir "$mnt" 2>/dev/null || true
    mnt=""
    # Recompress to a temp path, then atomically swap onto the original: never
    # delete the only good artifact before its replacement is fully written, so
    # a failed recompress can't lose the (expensive) build output.
    hdiutil convert "$rw" -format UDZO -imagekey zlib-level=9 -o "$out" >/dev/null \
        || { refresh_dmg_payload_cleanup "$rw" "$out" ""; die "hdiutil could not recompress $(basename "$rw")"; }
    mv -f "$out" "$dmg" \
        || { refresh_dmg_payload_cleanup "$rw" "$out" ""; die "could not move the recompressed image onto $(basename "$dmg")"; }
    refresh_dmg_payload_cleanup "$rw" "$out" ""
}

# ── The .app notarization stage (F5) ─────────────────────────────────────────
# Apple's documented ordering is: notarize and staple the .app, THEN build the
# disk image around the stapled app, then sign, notarize and staple the image.
# This script did only the second half. The only copy it ever stapled was the
# standalone build output, which is never shipped, so the `.app` inside every
# published DMG carried no ticket. Verified on all ten releases the 2026-08-02
# audit tested: `xcrun stapler validate` on the mounted app reports "does not
# have a ticket stapled to it", and `spctl` accepts it only through an ONLINE
# lookup against Apple's service. Apple's whole stated reason for stapling is to
# make that lookup unnecessary.
#
# THE CHEAP FIX DOES NOT EXIST. `stapler staple` writes the ticket INTO the
# bundle, so putting it in the copy inside the DMG means rewriting the image,
# which changes the image's own cdhash and voids both its signature and its
# ticket. Two submissions are required, and they cannot overlap, because the DMG
# has to be built from the already-stapled app.
#
# THE COST, stated because it is real: a release now waits for two Apple verdicts
# in sequence instead of one, and `--defer-notarization` can defer only the
# second. The app's verdict sits in the critical path of every release. On the
# v0.16.0 evidence that is a second window of up to eight hours. See ADR 0033,
# which records the amendment to ADR 0027 this makes.

# app_bundle_cdhash <app>: the code-directory hash codesign reports for <app>.
#
# This, and not a file hash, is what a notarization ticket is issued for, so it
# is what has to still be true before stapling. It is also the only workable
# choice: the submitted archive comes from `ditto -c -k`, which is not
# byte-reproducible, so re-archiving and comparing checksums would report false
# mismatches on an untouched bundle.
app_bundle_cdhash() {
    local app="$1" line
    line="$(codesign -dvvv "$app" 2>&1 | grep -m1 '^CDHash=' || true)"
    [ -n "$line" ] || return 1
    printf '%s' "${line#CDHash=}"
}

# notarize_app_zip_path: where the archive handed to notarytool lives.
#
# Deliberately `<app>.notarize.zip`, beside the bundle: it matches neither the
# `*.app` discovery nor the `*.app.tar.gz` one, so no later step can mistake it
# for the bundle or for the updater payload.
notarize_app_zip_path() {
    printf '%s.notarize.zip' "$APP_PATH"
}

# notarize_app_submit_and_persist: archive the signed .app, submit it, and record
# the resume handle before any waiting, exactly as the DMG stage does.
#
# This is the FIRST submission of a release, so it is where the updater pairing
# is captured and where the whole set is pinned. See
# notarize_capture_updater_pairing for why the second submission must inherit
# that pairing rather than re-read it from disk.
notarize_app_submit_and_persist() {
    local zip sha cdhash commit submitted_at
    zip="$(notarize_app_zip_path)"

    cdhash="$(app_bundle_cdhash "$APP_PATH")" \
        || die "could not read the code-directory hash of $APP_PATH. Without it there is no way to prove the bundle that gets stapled is the bundle Apple scanned."

    step "Archiving $(basename "$APP_PATH") for notarization"
    rm -f "$zip"
    # ditto rather than `zip`: it is what Apple documents for this, and it keeps
    # the symlinks and resource forks a bundle can carry.
    ditto -c -k --sequesterRsrc --keepParent "$APP_PATH" "$zip" \
        || die "could not archive $APP_PATH for notarization"
    sha="$(release_staging_sha256 "$zip")" \
        || die "could not hash $zip for the notarize resume handle"
    NOTARIZE_EXPECTED_SHA="$sha"
    commit="$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || true)"
    submitted_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

    notarize_capture_updater_pairing
    notarize_pin_submitted_set "$zip" "$sha"

    step "Submitting $(basename "$zip") to the Apple notary service"
    notarize_submit "$zip"

    RELEASE_NOTARIZE_APP_PATH="$APP_PATH" \
    RELEASE_NOTARIZE_APP_CDHASH="$cdhash" \
    RELEASE_NOTARIZE_UPDATER_TARBALL="$NOTARIZE_UPDATER_TARBALL" \
    RELEASE_NOTARIZE_UPDATER_TARBALL_SHA256="$NOTARIZE_UPDATER_TARBALL_SHA" \
    RELEASE_NOTARIZE_UPDATER_SIG_SHA256="$NOTARIZE_UPDATER_SIG_SHA" \
    release_notarize_write_state "$NOTARIZE_STATE_FILE" "$RELEASE_NOTARIZE_STAGE_APP" \
        "$NOTARIZE_SUBMISSION_ID" "$zip" "$sha" "$EFFECTIVE_VERSION" \
        "$commit" "$submitted_at" \
        || die "could not persist the notarize resume handle to $NOTARIZE_STATE_FILE"
    echo "    submission $NOTARIZE_SUBMISSION_ID (the .app): resume handle $NOTARIZE_STATE_FILE"
}

# notarize_restore_app_from_pin <zip-sha256>: put back the exact .app that was
# submitted, from its pinned archive. Non-zero when no usable pin survives.
notarize_restore_app_from_pin() {
    local pin parent base
    pin="$(notarize_find_pin "$1")"
    [ -n "$pin" ] || return 1
    parent="$(dirname "$APP_PATH")"
    base="$(basename "$APP_PATH")"
    echo "    NOTE: $base is no longer the bundle Apple scanned (a rebuild replaced it)."
    echo "          Recovering it from the pinned archive: $pin"
    rm -rf "${parent:?}/${base:?}"
    ditto -x -k "$pin" "$parent" || return 1
    [ -d "$APP_PATH" ] || return 1
}

# notarize_app_await_and_staple: poll the .app submission to a verdict, prove the
# bundle on disk is still the one Apple scanned, staple it, and prove the staple
# took without breaking the seal.
notarize_app_await_and_staple() {
    local expected_cdhash zip zip_sha sig="" actual

    expected_cdhash="$(release_notarize_field "$NOTARIZE_STATE_FILE" app_cdhash)" \
        || die "could not read the submitted app identity from $NOTARIZE_STATE_FILE"
    zip="$(release_notarize_field "$NOTARIZE_STATE_FILE" artifact_path)" \
        || die "could not read the submitted archive path from $NOTARIZE_STATE_FILE"
    zip_sha="$(release_notarize_field "$NOTARIZE_STATE_FILE" artifact_sha256)" \
        || die "could not read the submitted archive checksum from $NOTARIZE_STATE_FILE"

    notarize_await_verdict "$NOTARIZE_SUBMISSION_ID"

    # The whole submitted set has to survive the wait, not just the archive. The
    # updater payload is already packed by this point, and a concurrent rebuild
    # replaces it at the same fixed path it replaces everything else at (F3).
    if [ -n "$NOTARIZE_UPDATER_TARBALL" ]; then
        sig="$NOTARIZE_UPDATER_TARBALL.sig"
    fi
    assert_submitted_artifacts_are_intact \
        "the submitted .app archive" "$zip"                      "$zip_sha" \
        "the updater payload"        "$NOTARIZE_UPDATER_TARBALL" "$NOTARIZE_UPDATER_TARBALL_SHA" \
        "the updater signature"      "$sig"                      "$NOTARIZE_UPDATER_SIG_SHA"

    # The bundle itself is checked by CODE IDENTITY. A ticket is issued for a
    # cdhash, so this is the exact correctness condition, and it is both cheaper
    # and stricter than any file comparison could be.
    if [ -n "$expected_cdhash" ]; then
        actual="$(app_bundle_cdhash "$APP_PATH" 2>/dev/null || true)"
        if [ "$actual" != "$expected_cdhash" ]; then
            notarize_restore_app_from_pin "$zip_sha" \
                || die "REFUSING TO STAPLE: $APP_PATH is not the bundle Apple scanned.
       submitted: $expected_cdhash
       on disk:   ${actual:-(unreadable)}
       Another build replaced it while the submission was in flight, and no
       pinned archive survives to restore it from. A ticket issued for one code
       identity must never be attached to another. Rebuild."
            actual="$(app_bundle_cdhash "$APP_PATH" 2>/dev/null || true)"
            [ "$actual" = "$expected_cdhash" ] \
                || die "restored $APP_PATH from its pinned archive, but its code identity is ${actual:-(unreadable)} rather than the submitted $expected_cdhash."
        fi
    fi

    step "Stapling the notarization ticket to $(basename "$APP_PATH")"
    staple_idempotent "$APP_PATH"
    # Prove BOTH halves. The ticket being present in the copy that ships is the
    # entire point of this stage, and a staple that broke the seal would be worse
    # than no staple at all.
    xcrun stapler validate "$APP_PATH" >/dev/null 2>&1 \
        || die "stapler reported success for $APP_PATH but the ticket does not validate."
    codesign --verify --deep --strict "$APP_PATH" \
        || die "$APP_PATH no longer passes codesign --verify after stapling. The ticket goes to Contents/CodeResources, outside the sealed resource set, so this should not be possible. Refusing to build a DMG around a bundle macOS will not launch."
    echo "    $(basename "$APP_PATH") carries a stapled ticket, and its signature still verifies."
}

# notarize_adopt_app_submission <head-commit>: write a stage `app` resume handle
# for a submission that is ALREADY in flight but whose id was never persisted.
#
# The sibling of --adopt-submission, for the stage that now runs first. The
# window it covers is the same one: between notarytool returning an id and the
# handle reaching disk. Without it the only recovery from that window is a full
# rebuild, which is the cost the whole resume mechanism exists to avoid.
notarize_adopt_app_submission() {
    local head_commit="$1" app zip sha cdhash
    app="$(/usr/bin/find "$BUNDLE_DIR/macos" -maxdepth 1 -name '*.app' 2>/dev/null | head -1 || true)"
    [ -n "$app" ] \
        || die "--adopt-app-submission found no built .app under $BUNDLE_DIR/macos. Adoption needs the signed bundle that was submitted still on disk."
    APP_PATH="$app"
    zip="$(notarize_app_zip_path)"
    [ -f "$zip" ] \
        || die "--adopt-app-submission needs the archive that was submitted, and there is none at $zip. Without it nothing records what Apple actually scanned, and a rebuild during the wait could not be recovered from. Rebuild instead."
    sha="$(release_staging_sha256 "$zip")" || die "could not hash $zip"
    cdhash="$(app_bundle_cdhash "$app")" \
        || die "could not read the code-directory hash of $app"

    step "Adopting in-flight .app submission $ADOPT_APP_SUBMISSION for $(basename "$app")"
    notarize_capture_updater_pairing
    notarize_pin_submitted_set "$zip" "$sha"
    RELEASE_NOTARIZE_APP_PATH="$app" \
    RELEASE_NOTARIZE_APP_CDHASH="$cdhash" \
    RELEASE_NOTARIZE_UPDATER_TARBALL="$NOTARIZE_UPDATER_TARBALL" \
    RELEASE_NOTARIZE_UPDATER_TARBALL_SHA256="$NOTARIZE_UPDATER_TARBALL_SHA" \
    RELEASE_NOTARIZE_UPDATER_SIG_SHA256="$NOTARIZE_UPDATER_SIG_SHA" \
    release_notarize_write_state "$NOTARIZE_STATE_FILE" "$RELEASE_NOTARIZE_STAGE_APP" \
        "$ADOPT_APP_SUBMISSION" "$zip" "$sha" "$EFFECTIVE_VERSION" \
        "$head_commit" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        || die "could not write the notarize resume handle to $NOTARIZE_STATE_FILE"
    echo "    wrote $NOTARIZE_STATE_FILE (submitted_at records the adoption time)"
}

# run_app_notarize_resume <submission-id> <submitted-at>: pick a lost .app
# notarization back up. The DMG half runs afterwards, from the shared
# run_dmg_notarize_stage, so a resumed release and a fresh one build the image
# through the identical code.
run_app_notarize_resume() {
    local submission_id="$1" submitted_at="$2"
    APP_PATH="$(release_notarize_field "$NOTARIZE_STATE_FILE" app_path)" \
        || die "could not read the .app path from $NOTARIZE_STATE_FILE"
    [ -n "$APP_PATH" ] && [ -d "$APP_PATH" ] \
        || die "the .app this submission was made from is gone: '$APP_PATH'. Rebuild."
    # The DMG this build produced is still the pre-refresh one from `cargo tauri
    # build`; run_dmg_notarize_stage injects the stapled app into it below.
    DMG_PATH="$(release_dmg_find "$BUNDLE_DIR/dmg")" \
        || die "resuming the .app notarization needs the DMG this build produced (the reason is above)."

    notarize_credentials_present \
        || die "resuming the .app notarization needs APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID to ask Apple about submission $submission_id."
    [ -n "${APPLE_SIGNING_IDENTITY:-}" ] \
        || die "resuming the .app notarization needs APPLE_SIGNING_IDENTITY: the DMG built around the stapled app still has to be signed before it can be submitted."

    begin_step notarize "Resuming the .app notarization (submission $submission_id, submitted $submitted_at), then building the DMG around the stapled app, signing it, submitting it and stapling the ticket."
    step "Resuming .app notarization $submission_id, submitted $submitted_at"
    echo "    app:    $APP_PATH"
    echo "    handle: $NOTARIZE_STATE_FILE"
    notarize_announce_credentials
    notarize_app_await_and_staple
}

# ── Build the DMG around the stapled app, then notarize the DMG ──────────────
# One function so a fresh build and a resumed .app stage run the identical
# sequence. The caller opens the `notarize` cockpit step (the .app half runs
# inside it too, because the cockpit's step vocabulary has no id for a second
# notarization and inventing one it does not render would be worse); this
# function closes it.
run_dmg_notarize_stage() {
    step "Refreshing DMG payload and hiding .VolumeIcon.icns"
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
                # staged: a concurrent rebuild between submit and stage would
                # otherwise publish a DMG that the eventual ticket does not match.
                notarize_submit_and_persist
                assert_submitted_set_is_intact "$NOTARIZE_EXPECTED_SHA"
                DMG_NOTARIZED_STATE="false"
                step "Deferring the notary wait (--defer-notarization)"
                echo "    submission $NOTARIZE_SUBMISSION_ID is in flight; the DMG is signed but NOT stapled."
                echo "    Staging it so the release can publish now. Finish it later with:"
                echo "        scripts/release.sh --attach-notarized $EFFECTIVE_VERSION"
            else
                notarize_submit_and_wait
                # NOTARIZE_EXPECTED_SHA was recorded by notarize_submit_and_wait before
                # the wait; assert the DMG is still those bytes before stapling.
                staple_notarized_artifacts "$NOTARIZE_EXPECTED_SHA"
            fi
        elif [ "$RELEASE_MODE" = "1" ]; then
            die "release mode requires notarization but APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID are not all set. Refusing to ship an un-notarized DMG."
        else
            echo "    APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID not all set, so notarization is skipped."
        fi
    elif [ "$RELEASE_MODE" = "1" ]; then
        # The dev-identity fallback above can NEVER reach this branch as a pass: a
        # release asserts APPLE_SIGNING_IDENTITY at startup, and this is the second
        # place it refuses. A self-signed certificate is not a Developer ID.
        die "release mode requires signing but APPLE_SIGNING_IDENTITY is not set."
    else
        echo ""
        echo "NOTE: APPLE_SIGNING_IDENTITY not set, so the .dmg is UNSIGNED and not notarized."
        if [ -n "$BUNDLE_SIGNED_WITH" ]; then
            echo "      (The .app inside it is signed with the local dev identity, which is what"
            echo "      keeps your macOS permission grants across rebuilds. It is not a"
            echo "      Developer ID, so Gatekeeper still blocks this build elsewhere.)"
        fi
        echo "      Gatekeeper will block it on other Macs (right-click → Open to bypass locally)."
        echo "      Set the APPLE_* env vars to sign + notarize. See docs/desktop-app.md."
    fi
    # A DEFERRED run stapled nothing: the submission is still with Apple. Saying
    # "Notarized + stapled" here would make the Release Cockpit, the one surface an
    # operator checks to see whether a release is deferred, assert the opposite of
    # the truth. The step is genuinely finished (this run's notarize work is done),
    # so it still succeeds; only the summary tells which outcome it reached.
    # --attach-notarized emits the real completion later, through the resume path.
    if [ "$DMG_NOTARIZED_STATE" = "false" ]; then
        end_step notarize "Submitted to the Apple notary service and DEFERRED: the DMG is signed but NOT stapled; finish with release.sh --attach-notarized $EFFECTIVE_VERSION."
    else
        end_step notarize "Notarized + stapled the DMG and the .app."
    fi
}

# run_notarize_resume — pick a lost notarization back up. NO build, NO codesign,
# NO re-submit: the DMG is already signed on disk and already with Apple, so this
# polls for the verdict, staples, and runs the same finalize tail a fresh build
# does. This is what makes a Phase A survive losing the process waiting on Apple.
run_notarize_resume() {
    local head_commit stage submission_id submitted_at

    [ -n "$EFFECTIVE_VERSION" ] \
        || die "resuming notarization needs a version — no RELEASE file at $REPO_ROOT/RELEASE."
    head_commit="$(git -C "$REPO_ROOT" rev-parse HEAD)" \
        || die "cannot read git HEAD of $REPO_ROOT to verify the resume handle."

    if [ -n "$ADOPT_SUBMISSION" ]; then
        notarize_adopt_submission "$head_commit"
    fi
    if [ -n "$ADOPT_APP_SUBMISSION" ]; then
        notarize_adopt_app_submission "$head_commit"
    fi

    release_notarize_resumable "$NOTARIZE_STATE_FILE" "$head_commit" \
        || die "cannot resume notarization for $EFFECTIVE_VERSION (see the reason above)."

    stage="$(release_notarize_field "$NOTARIZE_STATE_FILE" stage)" \
        || die "could not read the notarization stage from $NOTARIZE_STATE_FILE"
    submission_id="$(release_notarize_field "$NOTARIZE_STATE_FILE" submission_id)" \
        || die "could not read the submission id from $NOTARIZE_STATE_FILE"
    # Mirror it into the global the closing report reads, so a deferred resume
    # names the submission that is still with Apple instead of printing a blank.
    NOTARIZE_SUBMISSION_ID="$submission_id"
    submitted_at="$(release_notarize_field "$NOTARIZE_STATE_FILE" submitted_at)" \
        || die "could not read the submit time from $NOTARIZE_STATE_FILE"
    # Load the updater payload this release's FIRST submission was paired with,
    # before either branch below can assert against it (F3).
    notarize_carry_updater_pairing_forward

    # A release makes two submissions in sequence, so a resume has to know which
    # half it is picking up. The .app half runs on into the DMG half through the
    # same run_dmg_notarize_stage a fresh build uses, so the two cannot drift.
    #
    # --defer-notarization cannot reach this branch as a shortcut, and that is
    # not an oversight: the DMG is BUILT FROM the stapled app, so there is nothing
    # to stage until the app's verdict lands. Deferral defers the DMG's verdict,
    # never the app's.
    if [ "$stage" = "$RELEASE_NOTARIZE_STAGE_APP" ]; then
        if [ "$DEFER_NOTARIZATION" = "1" ]; then
            step "The .app notarization cannot be deferred; waiting for its verdict, then deferring the DMG's"
            echo "    The DMG is built from the stapled .app, so there is nothing to stage"
            echo "    until Apple answers on submission $submission_id. Only the DMG's own"
            echo "    verdict is deferred."
        fi
        run_app_notarize_resume "$submission_id" "$submitted_at"
        run_dmg_notarize_stage
        finalize_release_artifacts
        return 0
    fi

    DMG_PATH="$(release_notarize_field "$NOTARIZE_STATE_FILE" artifact_path)" \
        || die "could not read the DMG path from $NOTARIZE_STATE_FILE"
    NOTARIZE_EXPECTED_SHA="$(release_notarize_field "$NOTARIZE_STATE_FILE" artifact_sha256)" \
        || die "could not read the submitted checksum from $NOTARIZE_STATE_FILE"
    # The path test below is a necessary condition and never was a sufficient one:
    # it says the recorded DMG lives in this tree, and says nothing about whether
    # the .app.tar.gz beside it belongs to the same build. The pairing loaded
    # above is what answers that, and every gate from here on re-derives its
    # verdict by re-hashing those exact bytes (F3).
    # Staging pairs the recorded DMG with the .app.tar.gz + .sig found under
    # BUNDLE_DIR, so refuse a handle whose DMG lives somewhere else: the pairing
    # checks would then be comparing this tree's artifacts against another tree's
    # submission.
    case "$DMG_PATH" in
        "$BUNDLE_DIR/dmg/"*) ;;
        *) die "the resume handle records a DMG outside this tree's bundle dir ($DMG_PATH is not under $BUNDLE_DIR/dmg/) — resume from the tree that built it." ;;
    esac
    APP_PATH="$(/usr/bin/find "$BUNDLE_DIR/macos" -maxdepth 1 -name '*.app' 2>/dev/null | head -1 || true)"

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
        assert_submitted_set_is_intact "$NOTARIZE_EXPECTED_SHA"
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
    staple_notarized_artifacts "$NOTARIZE_EXPECTED_SHA"
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
    # The .app archive is spent either way. Both notary stages are behind us by
    # here (the DMG cannot have been built without the app's ticket), the handle
    # names the DMG stage, and the pinned copy is what a recovery would reach for,
    # so the working copy is just ~70MB of litter in target/ per build.
    if [ -n "${APP_PATH:-}" ]; then
        rm -f "$(notarize_app_zip_path)"
    fi

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
        --notarize-deadline)
            [ $# -ge 2 ] || die "--notarize-deadline requires $(release_deadline_accepted_forms)"
            NOTARIZE_DEADLINE="$(release_deadline_parse "$2")" \
                || die "--notarize-deadline: could not read '$2' (see above)."
            shift 2 ;;
        --allow-pending-notarization) ALLOW_PENDING_NOTARIZATION=1; shift ;;
        --adopt-submission)
            [ $# -ge 2 ] || die "--adopt-submission requires a notary submission UUID"
            ADOPT_SUBMISSION="$2"; DO_RESUME_NOTARIZE=1; shift 2 ;;
        --adopt-app-submission)
            [ $# -ge 2 ] || die "--adopt-app-submission requires a notary submission UUID"
            ADOPT_APP_SUBMISSION="$2"; DO_RESUME_NOTARIZE=1; shift 2 ;;
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
if [ -n "$ADOPT_APP_SUBMISSION" ]; then
    release_notarize_valid_submission_id "$ADOPT_APP_SUBMISSION" \
        || die "--adopt-app-submission expects a notary submission UUID (8-4-4-4-12 hex), got '$ADOPT_APP_SUBMISSION'"
fi
# One handle, one outstanding submission. Adopting both would write the .app
# handle over the DMG one (or the reverse, depending on order) and silently throw
# a live submission away.
if [ -n "$ADOPT_SUBMISSION" ] && [ -n "$ADOPT_APP_SUBMISSION" ]; then
    die "--adopt-submission and --adopt-app-submission name the two halves of one release's notarization, and only one can be outstanding at a time. Adopt the one that is actually in flight."
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

# --notarize-deadline bounds a WAIT, so it needs a run that waits. The three
# refusals below are each a different mistake, and each is worth naming rather
# than silently ignoring a flag whose whole purpose is to change what happens
# hours from now.
if [ -n "$NOTARIZE_DEADLINE" ]; then
    # The attach check comes FIRST because --release-attach satisfies neither
    # condition, and "cannot be combined with an upload" tells the operator
    # something "only applies to a build" does not.
    [ "$DO_ATTACH" != "1" ] \
        || die "--notarize-deadline cannot be combined with --release / --release-attach. Those upload in this same process, so there is no later run to hand an outstanding verdict to."
    [ "$DO_BUILD" = "1" ] \
        || die "--notarize-deadline only applies to a build (--release-build, with or without --resume-notarize); nothing else here waits on Apple."
    [ "$DEFER_NOTARIZATION" != "1" ] \
        || die "--notarize-deadline and --defer-notarization are alternatives, not a pair. Deferring never waits for the DMG's verdict at all, so there is no wait for a deadline to bound. Deferring publishes behind a 'notarization pending' banner; a deadline publishes nothing."
fi

# ── --release-attach (no build): verify the staged artifacts and upload them ──
# This path does NO build, NO codesign, NO notarize — it only attaches artifacts
# a prior --release-build already produced + verified. It therefore needs neither
# the Apple/Tauri signing creds nor the cargo/tauri/npm build TOOLING; it needs
# `gh`, a valid staging dir, and (since the updater-payload gate landed) the
# system `codesign` to re-verify the staged payload before publishing it. That
# last one keeps this path macOS-bound in practice, which costs nothing: a
# release is macOS-only end to end, and require_release_signing_credentials
# refuses a non-Darwin host outright. Handled before the build preamble so the
# manifest guard still fails fast offline.
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

# release_dmg_find, not a bare `find … | head -1`: that returned DIRECTORY order
# over everything matching `*.dmg`, so a .rw.dmg / .zlib.dmg left by a run killed
# mid-refresh could be adopted as the release DMG and then signed, notarized,
# stapled and published. The version-stamp guard below cannot catch that, because
# the leftovers carry the same version string (F4). The adopt path has had this
# exclusion since it was written; this is the site that did not.
DMG_PATH="$(release_dmg_find "$BUNDLE_DIR/dmg")" \
    || die "could not identify the .dmg this build produced (the reason is above)."
APP_PATH="$(/usr/bin/find "$BUNDLE_DIR/macos" -maxdepth 1 -name '*.app' 2>/dev/null | head -1 || true)"
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
# sign_app_bundle <app> <identity> [<codesign-arg>…]: sign every Mach-O file in
# the bundle inside-out, then the bundle itself, with <identity>. The extra args
# are passed to every codesign call, which is how the two callers differ: the
# Developer ID path needs `--options runtime --timestamp` for notarization, the
# local dev-identity fallback deliberately needs neither (see the call site).
sign_app_bundle() {
    local app="$1" identity="$2"
    shift 2
    # macOS ships bash 3.2, where expanding an EMPTY named array under `set -u`
    # is an unbound-variable error, so every use below is written
    # ${sign_args[@]+"${sign_args[@]}"}. Both callers pass flags today; a future
    # one that passes none must not blow up halfway through ~200 codesigns.
    local -a sign_args=("$@")
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

    step "Signing ${#macho_files[@]} Mach-O binaries inside-out with $identity"
    for path in "${macho_files[@]}"; do
        codesign --force ${sign_args[@]+"${sign_args[@]}"} --sign "$identity" "$path" \
            || die "codesign failed for $path"
    done

    # Sign the outer .app LAST. Keep --deep as belt-and-suspenders (re-seals any
    # nested bundle), but the loose payload above is what makes notarization pass.
    codesign --force --deep ${sign_args[@]+"${sign_args[@]}"} --sign "$identity" "$app" \
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

# repack_updater_payload: rebuild Lucidos.app.tar.gz from the app we just signed,
# and re-sign it.
#
# THE v0.19.0 BUG. `cargo tauri build` packed this tarball several steps ago,
# from the app as the bundler left it, and the build above deliberately ran with
# APPLE_SIGNING_IDENTITY stripped from its env so Tauri would skip its own
# (insufficient) codesign pass. So the tarball on disk holds an UNSIGNED bundle.
# refresh_dmg_payload re-injects the signed app into the DMG a few lines below,
# which is why the DMG has always been correct; nothing did the same for the
# updater payload, so every auto-update since replaced a notarized Developer ID
# app with an ad-hoc one whose designated requirement is a bare cdhash. macOS TCC
# keys grants on code identity, so each update silently destroyed every
# permission the user had ever granted.
#
# Ordering: this MUST run after sign_app_bundle and before refresh_dmg_payload,
# so it packs the signed bundle and leaves the DMG path untouched.
#
# Pre-staple by design, matching the accepted cost already documented for the
# deferred-DMG mode: notarization has not happened yet, so the payload is
# Developer ID signed but carries no stapled ticket. Repacking after the staple
# instead would make the tarball's contents depend on whether the release was
# deferred (--defer-notarization never staples at all), and Gatekeeper never
# assesses the tarball anyway, since a payload the updater downloads carries no
# quarantine xattr.
repack_updater_payload() {
    local tarball
    tarball="$(/usr/bin/find "$BUNDLE_DIR/macos" -maxdepth 1 -name '*.app.tar.gz' 2>/dev/null | head -1 || true)"
    if [ -z "$tarball" ]; then
        echo "    (no .app.tar.gz produced by this build, so there is no updater payload to repack.)"
        return 0
    fi

    step "Repacking the updater payload from the SIGNED app: $(basename "$tarball")"
    updater_payload_repack "$APP_PATH" "$tarball" \
        || die "could not repack the updater payload from $APP_PATH"

    # The .sig covers the OLD bytes now. Regenerating it is not optional: a
    # signature that does not match makes every updater reject the update. Tauri
    # only emits a .sig when the updater key is set, and a build with no key
    # never had one to invalidate.
    if [ -f "$tarball.sig" ]; then
        updater_payload_resign "$tarball" \
            || die "repacked $(basename "$tarball") but could not re-sign it; the stale .sig would make every updater reject the update"
        echo "    re-signed $(basename "$tarball").sig over the repacked bytes"
    else
        echo "    (no .app.tar.gz.sig on disk, so no updater key was set; nothing to re-sign.)"
    fi
}

# Which identity signed the bundle, if any. Empty means Tauri's ad-hoc output was
# left alone, which is the only case where the updater payload is left stale too.
BUNDLE_SIGNED_WITH=""
# The summary the `codesign` step closes with. Deferred into a variable because
# the step does NOT end at the bottom of this if/elif: it stays open across the
# updater repack below, so a repack failure reds the cockpit instead of stalling
# it (see the end_step call after the repack).
CODESIGN_STEP_SUMMARY=""

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    begin_step codesign "Signing gateway + engine + app + bundled Postgres tree with Developer ID, then repacking the updater payload from the signed bundle."
    step "Codesigning bundled gateway + engine + app"
    # --options runtime (hardened runtime) and --timestamp (a secure timestamp
    # from Apple's TSA) are both REQUIRED for notarization.
    sign_app_bundle "$APP_PATH" "$APPLE_SIGNING_IDENTITY" --options runtime --timestamp
    BUNDLE_SIGNED_WITH="$APPLE_SIGNING_IDENTITY"
    CODESIGN_STEP_SUMMARY="Codesigned the bundle (gateway + engine + app + ~200 loose Postgres Mach-O files) with Developer ID, and repacked the updater payload from it."
elif lucidos_signing_identity_ready; then
    # LOCAL BUILDS ONLY. Release mode never reaches here: it asserts
    # APPLE_SIGNING_IDENTITY at startup and dies below if signing was skipped, so
    # the self-signed dev identity can never satisfy a release. The staging gate
    # is the second lock: a dev-identity payload has no Team Identifier, so
    # updater_payload_assert_developer_id refuses it.
    #
    # What this buys: Tauri's ad-hoc output has a cdhash-anchored designated
    # requirement, so every local rebuild of the .app is a NEW code identity and
    # macOS re-prompts for (and discards) every TCC grant. The dev identity is a
    # stable certificate, so the requirement becomes `identifier "com.lucidos.app"
    # and certificate leaf = H"…"` and one Allow click sticks across rebuilds.
    # This is the same fix scripts/lib/codesign.sh already applies to the dev
    # engine binary, applied here to the packaged bundle.
    #
    # NEITHER --options runtime NOR --timestamp here, deliberately:
    #   • hardened runtime exists to satisfy notarization, which this path never
    #     does. Worse, library validation under the hardened runtime matches
    #     loaded code by Team Identifier, and a self-signed certificate has none,
    #     so enabling it risks the bundled Postgres dylibs failing to load at
    #     exactly the moment the developer is trying to test the packaged app.
    #   • a secure timestamp means a network round trip to Apple PER FILE, and
    #     there are ~200 of them, for a certificate no one else trusts.
    # The certificate-anchored designated requirement, which is the entire point,
    # depends on neither.
    begin_step codesign "Signing the app with the stable dev identity (local build; no Developer ID configured)."
    step "Codesigning with the stable dev identity ($LUCIDOS_SIGNING_IDENTITY)"
    lucidos_ensure_keychain_in_search_list
    security unlock-keychain -p "$LUCIDOS_SIGNING_KC_PASS" "$LUCIDOS_SIGNING_KEYCHAIN" 2>/dev/null || true
    sign_app_bundle "$APP_PATH" "$LUCIDOS_SIGNING_IDENTITY" \
        --timestamp=none --keychain "$LUCIDOS_SIGNING_KEYCHAIN"
    BUNDLE_SIGNED_WITH="$LUCIDOS_SIGNING_IDENTITY"
    CODESIGN_STEP_SUMMARY="Codesigned the bundle with the stable dev identity (local build, not notarizable) and repacked the updater payload from it."
    echo "    NOTE: signed with the LOCAL dev identity, not Developer ID. Gatekeeper"
    echo "          still blocks this build on other Macs, but your macOS permission"
    echo "          grants now survive a rebuild."
else
    echo ""
    echo "NOTE: no APPLE_SIGNING_IDENTITY and no dev signing identity, so the .app is"
    echo "      left ad-hoc signed. Its code identity changes on every rebuild, so"
    echo "      macOS will re-prompt for every permission. Fix once with:"
    echo "      ./scripts/dev-codesign-setup.sh"
fi

# ── 5b2. repack the updater payload from the signed app ─────────────────────
# Gated on the bundle actually having been signed, not literally on
# APPLE_SIGNING_IDENTITY: the dev-identity branch above signs it too, and a
# tarball that disagrees with the .app sitting next to it is the whole bug. An
# unsigned local build keeps today's behaviour, where the tarball and the app
# match because neither is signed.
#
# This runs INSIDE the still-open `codesign` step, and that is load-bearing:
# die/on_err emit ReleaseStepFailed only while CURRENT_STEP is set, so a repack
# failure in the gap between two steps would exit non-zero with NO event and the
# Release Cockpit would stall on a green `codesign` forever. That silent stall is
# the exact failure the `set -E` + on_err rationale at the top of this file
# exists to prevent. Signing the bundle and making the updater payload match it
# are one unit of work, so one step covers both.
if [ -n "$BUNDLE_SIGNED_WITH" ]; then
    repack_updater_payload
    end_step codesign "$CODESIGN_STEP_SUMMARY"
fi

# ── 5c. notarize + staple the .app, then build the DMG around it ────────────
# Apple's ordering (see "The .app notarization stage" near the top of this
# file). The cockpit step is opened here and closed by run_dmg_notarize_stage,
# so BOTH notary submissions report under one `notarize` step.
#
# Gated on the same credentials the DMG's own notarization is gated on. A local
# build with the full Apple credential set is deliberately reproducing the
# release and gets both submissions; one without them gets neither. A third
# behaviour keyed on release mode would be a special case with no user.
begin_step notarize "Notarizing and stapling the .app, then refreshing the DMG payload around it, signing the DMG, submitting it, polling for the verdict and stapling the ticket."
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && notarize_credentials_present; then
    notarize_announce_credentials
    notarize_app_submit_and_persist
    notarize_app_await_and_staple
fi
run_dmg_notarize_stage

# ── 6b–7. stage → drop the resume handle → tarball → upload → report ─────────
# Shared with the resume path so the two can't drift (see
# finalize_release_artifacts near the top of this script).
finalize_release_artifacts
