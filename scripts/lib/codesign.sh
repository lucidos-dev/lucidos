#!/bin/bash
# Stable code-signing of the dev `lucidos-engine` binary so macOS TCC (privacy)
# grants persist across rebuilds.
#
# Why this exists: a `cargo build` binary is `adhoc, linker-signed`. Its CDHash
# changes on every rebuild, and macOS TCC keys permission grants by the
# responsible process's code identity — so after each rebuild TCC discards the
# prior grant and re-prompts ("lucidos-engine would like to access ..."). Signing
# the binary with a *stable* self-signed certificate gives it a rebuild-stable
# Designated Requirement (`identifier "lucidos-engine" and certificate leaf =
# H"..."` — no CDHash), so one Allow click sticks across every rebuild.
#
# The one-time setup (creating + trusting the cert) lives in
# scripts/dev-codesign-setup.sh. This file is the shared contract: constants +
# the build-time signing step + the readiness probe, sourced by both that setup
# script and scripts/lib/workspace.sh.

# Display name of the dev signing identity (as `security find-identity` lists it).
LUCIDOS_SIGNING_IDENTITY="Lucidos Dev Code Signing"
# Dedicated keychain, separate from the login keychain: because we own its
# password we can run `set-key-partition-list` non-interactively, so codesign
# never pops a "wants to use a key" GUI prompt at build time. It holds only the
# self-signed dev code-signing cert — no real secrets — so the password living in
# this file is acceptable for local dev tooling.
LUCIDOS_SIGNING_KEYCHAIN="$HOME/Library/Keychains/lucidos-dev-signing.keychain-db"
LUCIDOS_SIGNING_KC_PASS="lucidos-dev"
# Signing identifier. Kept equal to the binary's basename so the macOS TCC prompt
# keeps reading "lucidos-engine" (unchanged from today), while the certificate
# leaf — not the CDHash — anchors the rebuild-stable Designated Requirement.
LUCIDOS_SIGNING_IDENTIFIER="lucidos-engine"

# True only on macOS (codesign/TCC are macOS-only; no-op everywhere else).
_lucidos_is_macos() { [ "$(uname -s)" = "Darwin" ]; }

# Succeeds when the dev signing identity exists and is valid for code signing
# (i.e. the cert has been trusted). Used to decide whether to sign and to make
# setup idempotent.
lucidos_signing_identity_ready() {
    _lucidos_is_macos || return 1
    [ -f "$LUCIDOS_SIGNING_KEYCHAIN" ] || return 1
    security find-identity -v -p codesigning "$LUCIDOS_SIGNING_KEYCHAIN" 2>/dev/null \
        | grep -qF "$LUCIDOS_SIGNING_IDENTITY"
}

# Ensure the dev signing keychain is in the user keychain search list.
#
# This is the load-bearing step that the original setup omitted: `codesign
# --sign <name>` resolves the identity through the *search list*, NOT through the
# `--keychain` flag (that flag does not scope identity-name resolution). A
# keychain that is created + trusted but never added to the search list yields
# "no identity found", and because sign_engine_binary is best-effort every build
# then silently falls back to an ad-hoc binary — whose CDHash changes each
# rebuild, so macOS TCC re-prompts forever. Note that
# `find-identity -p codesigning "$KEYCHAIN"` (explicit) still reports the
# identity as valid in that broken state, which is why the omission went
# unnoticed.
#
# Idempotent and self-healing: appends our keychain only when missing, preserving
# the existing search list. Called from both setup and the build-time signer so
# existing broken installs repair themselves on the next `-b` build. macOS-only,
# best-effort (never blocks the build).
lucidos_ensure_keychain_in_search_list() {
    _lucidos_is_macos || return 0
    [ -f "$LUCIDOS_SIGNING_KEYCHAIN" ] || return 0
    # Already present? (search-list entries are printed quoted, full path.)
    if security list-keychains -d user 2>/dev/null | grep -qF "$LUCIDOS_SIGNING_KEYCHAIN"; then
        return 0
    fi
    # Rebuild the list = existing entries + ours. Use an array so paths with
    # spaces survive; `-s` takes the complete desired list as arguments.
    local -a list=()
    local kc
    while IFS= read -r kc; do
        [ -n "$kc" ] && list+=("$kc")
    done < <(security list-keychains -d user 2>/dev/null | sed -E 's/^[[:space:]]*"(.*)"[[:space:]]*$/\1/')
    list+=("$LUCIDOS_SIGNING_KEYCHAIN")
    security list-keychains -d user -s "${list[@]}" >/dev/null 2>&1 || true
}

# Sign the engine binary with the stable dev identity. Best-effort: prints a hint
# and returns 0 on any problem so a signing issue never blocks the dev build.
# $1 = path to the engine binary.
sign_engine_binary() {
    local bin="$1"
    _lucidos_is_macos || return 0
    [ -n "$bin" ] && [ -f "$bin" ] || return 0
    if ! lucidos_signing_identity_ready; then
        echo "ℹ️  Dev code-signing identity not set up — engine left ad-hoc signed."
        echo "    macOS may re-prompt for permissions after each rebuild. Fix once with:"
        echo "    ./scripts/dev-codesign-setup.sh"
        return 0
    fi
    # Self-heal: without our keychain in the search list, the codesign below
    # fails with "no identity found" and silently leaves the binary ad-hoc.
    lucidos_ensure_keychain_in_search_list
    security unlock-keychain -p "$LUCIDOS_SIGNING_KC_PASS" "$LUCIDOS_SIGNING_KEYCHAIN" 2>/dev/null || true
    if codesign --force \
        --identifier "$LUCIDOS_SIGNING_IDENTIFIER" \
        --keychain "$LUCIDOS_SIGNING_KEYCHAIN" \
        --sign "$LUCIDOS_SIGNING_IDENTITY" \
        "$bin" 2>/dev/null; then
        echo "Signed engine with stable dev identity ($LUCIDOS_SIGNING_IDENTITY)."
    else
        echo "⚠️  Failed to sign engine with dev identity — left ad-hoc signed."
        echo "    Re-run ./scripts/dev-codesign-setup.sh if the identity was removed."
    fi
    return 0
}
