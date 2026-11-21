#!/bin/bash
# One-time setup: create a stable self-signed code-signing identity used to sign
# the dev `lucidos-engine` binary, so macOS remembers privacy (TCC) permissions
# across rebuilds. Without it, every `cargo build` changes the binary's CDHash
# and macOS re-prompts ("lucidos-engine would like to access ...") after each
# rebuild. See scripts/lib/codesign.sh for the full rationale.
#
# Run this ONCE. The only interactive step is a single macOS GUI prompt asking
# for your login password to trust the certificate for code signing. The prompt
# keeps saying "lucidos-engine" (not "Claude") — this only stops the re-prompting.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/codesign.sh
source "$SCRIPT_DIR/lib/codesign.sh"

if ! _lucidos_is_macos; then
    echo "Not macOS — code signing is only needed for macOS TCC. Nothing to do."
    exit 0
fi

if lucidos_signing_identity_ready; then
    # Heal the search-list registration in case an earlier setup created + trusted
    # the keychain but never added it to the search list (the bug this fixes):
    # codesign resolves --sign by name through the search list, so without this
    # every build silently signs ad-hoc despite the identity being "ready".
    lucidos_ensure_keychain_in_search_list
    echo "Dev code-signing identity '$LUCIDOS_SIGNING_IDENTITY' is already set up."
    echo "Nothing to do. Rebuild the engine to (re)sign it: ./scripts/web-dev.sh -w <ws> -b"
    exit 0
fi

echo "Setting up a stable self-signed code-signing identity for the dev engine."
echo "This lets macOS remember privacy (TCC) permissions across rebuilds — you"
echo "click Allow once instead of after every rebuild."
echo

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Ephemeral passphrase for the in-flight .p12 bundle. macOS `security import`
# rejects an empty-password PKCS#12 ("MAC verification failed"), so this must be
# non-empty. The .p12 exists only inside $TMP and is deleted on exit, so the
# value is throwaway and never persisted.
P12_PASS="lucidos-transient"

# 1) Self-signed certificate with the code-signing extended key usage.
# stderr is captured and surfaced only on failure (set -e aborts on a non-zero
# openssl), so a cert-generation problem is reported instead of a silent exit.
if ! openssl req -x509 -newkey rsa:2048 -keyout "$TMP/key.pem" -out "$TMP/cert.pem" \
    -days 3650 -nodes -subj "/CN=$LUCIDOS_SIGNING_IDENTITY" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning" >/dev/null 2>"$TMP/openssl.err"; then
    echo "ERROR: failed to generate the signing certificate:" >&2
    cat "$TMP/openssl.err" >&2
    exit 1
fi
# Force the legacy SHA1-based PKCS#12 PBE algorithms (3DES for the key and certs,
# SHA1 MAC). macOS `security import` only accepts these; it rejects the AES-256 +
# SHA-256-MAC bundle that OpenSSL 3.x emits by default, failing with "MAC
# verification failed during PKCS12 import (wrong password?)". Because the script
# inherits whatever `openssl` is first on PATH, a developer with Homebrew's
# OpenSSL 3.x (not the system LibreSSL) would otherwise hit that failure here —
# and under `set -e` it aborts the script after the keychain is created but before
# anything is imported, leaving an empty keychain. These explicit flags are
# accepted by both LibreSSL and OpenSSL 3.x, so the export is portable across
# either; the OpenSSL-3-only `-legacy` shorthand is avoided because LibreSSL
# rejects it as an unknown option.
if ! openssl pkcs12 -export -macalg sha1 -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES \
    -out "$TMP/id.p12" -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
    -passout "pass:$P12_PASS" -name "$LUCIDOS_SIGNING_IDENTITY" >/dev/null 2>"$TMP/openssl.err"; then
    echo "ERROR: failed to bundle the signing certificate into a .p12:" >&2
    cat "$TMP/openssl.err" >&2
    exit 1
fi

# 2) Dedicated keychain we own (known password → non-interactive key access).
if [ ! -f "$LUCIDOS_SIGNING_KEYCHAIN" ]; then
    security create-keychain -p "$LUCIDOS_SIGNING_KC_PASS" "$LUCIDOS_SIGNING_KEYCHAIN"
fi
security set-keychain-settings "$LUCIDOS_SIGNING_KEYCHAIN"   # no auto-lock timeout
security unlock-keychain -p "$LUCIDOS_SIGNING_KC_PASS" "$LUCIDOS_SIGNING_KEYCHAIN"

# 3) Import key+cert into that keychain; authorize codesign to use the key, and
#    set the partition list so codesign won't prompt for key access at build time.
security import "$TMP/id.p12" -k "$LUCIDOS_SIGNING_KEYCHAIN" -P "$P12_PASS" -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple: -s \
    -k "$LUCIDOS_SIGNING_KC_PASS" "$LUCIDOS_SIGNING_KEYCHAIN" >/dev/null 2>&1 || true

# 4) Trust the cert for code signing (user trust domain, code-signing policy
#    only). This is the single interactive step: macOS asks for your login
#    password via a GUI dialog. codesign refuses to use an untrusted identity, so
#    this is required.
echo "macOS will now ask for your login password to trust the certificate for"
echo "code signing — this is the only interactive step."
security add-trusted-cert -r trustRoot -p codeSign -k "$LUCIDOS_SIGNING_KEYCHAIN" "$TMP/cert.pem"

# 5) Register the keychain in the user search list. Without this, codesign cannot
#    resolve the identity by name at build time ("no identity found"), so every
#    build would silently fall back to ad-hoc signing — the original bug.
lucidos_ensure_keychain_in_search_list

echo
if lucidos_signing_identity_ready; then
    echo "✅ Done. '$LUCIDOS_SIGNING_IDENTITY' is ready."
    echo "   Rebuild the engine (./scripts/web-dev.sh -w <ws> -b) — it is now signed"
    echo "   automatically on every build. Click Allow once on the next macOS"
    echo "   permission prompt and it will persist across rebuilds."
else
    echo "⚠️  Identity not detected as valid after setup."
    echo "   Inspect with: security find-identity -v -p codesigning \"$LUCIDOS_SIGNING_KEYCHAIN\""
    exit 1
fi
