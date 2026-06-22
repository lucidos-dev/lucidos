#!/usr/bin/env bash
# tauri_signing_key.sh — resolve the Tauri updater signing key for the bundler.
#
# Tauri's bundler reads ONLY the literal env var TAURI_SIGNING_PRIVATE_KEY and
# expects the base64 key CONTENTS there (or a path it auto-detects). We don't
# rename the var Tauri reads. Instead the release path takes the key via the
# self-documenting TAURI_SIGNING_PRIVATE_KEY_PATH (the FILE PATH to the updater
# key, e.g. ~/.tauri/lucidos-updater.key), loads its contents, and exports them
# under the name Tauri reads.
#
# Sourced by scripts/build-dmg.sh (which actually runs `cargo tauri build`) and
# scripts/lib/release_signing.sh (the credential preflight). The functions are
# pure shell — only env vars + the key file — so the preflight can validate
# resolution in a subshell without mutating its own environment.

# resolve_tauri_signing_private_key — make TAURI_SIGNING_PRIVATE_KEY hold the key
# CONTENTS for the `cargo tauri build` / signer subprocess.
#
# Resolution order:
#   1. TAURI_SIGNING_PRIVATE_KEY_PATH set → read that file, export its contents
#      as TAURI_SIGNING_PRIVATE_KEY (overriding any inherited value).
#   2. else TAURI_SIGNING_PRIVATE_KEY set → leave it untouched (back-compat: it
#      already holds the contents, or a path Tauri auto-detects).
#   3. else → nothing to do (the caller's missing-credential gate reports it).
#
# Returns non-zero (with the reason on stderr) ONLY when a PATH was given but the
# file is missing / unreadable / empty — an explicit misconfiguration worth
# failing fast on, rather than discovering it after a full build produces no .sig.
resolve_tauri_signing_private_key() {
    local key_path="${TAURI_SIGNING_PRIVATE_KEY_PATH:-}"
    [ -n "$key_path" ] || return 0

    # Expand a leading ~/ ourselves: the var is often set in a config file (not a
    # shell), where tilde expansion never happened.
    case "$key_path" in
        "~"/*) key_path="$HOME/${key_path#"~"/}" ;;
    esac

    if [ ! -f "$key_path" ]; then
        echo "ERROR: TAURI_SIGNING_PRIVATE_KEY_PATH points at '$key_path', which is not a file." >&2
        return 1
    fi
    if [ ! -r "$key_path" ]; then
        echo "ERROR: TAURI_SIGNING_PRIVATE_KEY_PATH file '$key_path' is not readable." >&2
        return 1
    fi
    local contents
    contents="$(cat "$key_path")"
    if [ -z "$contents" ]; then
        echo "ERROR: TAURI_SIGNING_PRIVATE_KEY_PATH file '$key_path' is empty." >&2
        return 1
    fi

    export TAURI_SIGNING_PRIVATE_KEY="$contents"
    return 0
}
