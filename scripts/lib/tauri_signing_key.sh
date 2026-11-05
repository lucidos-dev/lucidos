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
# Sourced by scripts/build-dmg.sh (which actually runs `cargo tauri build`),
# scripts/lib/updater_payload.sh (which re-signs the repacked updater tarball) and
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

# tauri_signing_key_resolved: print the updater key as the SIGNER should receive
# it, either a path to the key file or the key contents. Empty output + non-zero
# when no key is configured at all.
#
# Deliberately non-mutating (no export), unlike resolve_tauri_signing_private_key
# above, which exists to set up the `cargo tauri build` subprocess. The preflight
# validates inside a subshell and the repack runs mid-build, so both need to ask
# "what key would be used?" without changing their own environment.
tauri_signing_key_resolved() {
    local key
    if [ -n "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" ]; then
        key="${TAURI_SIGNING_PRIVATE_KEY_PATH}"
        # Expand a leading ~/ ourselves: the var is usually set in a config file,
        # where no shell ever expanded it.
        case "$key" in "~"/*) key="$HOME/${key#"~"/}" ;; esac
    else
        key="${TAURI_SIGNING_PRIVATE_KEY:-}"
    fi
    [ -n "$key" ] || return 1
    printf '%s' "$key"
}

# tauri_signer_sign_file <file>: sign <file> with the updater key, leaving the
# detached signature at <file>.sig (which is what Tauri's updater verifies).
#
# THE single `cargo tauri signer sign` call site in the repo, and it has to stay
# that way. Two callers need it and they must not drift: release_signing.sh's
# throwaway preflight test-sign (which proves the key CAN sign before a release
# does anything destructive) and updater_payload.sh's re-sign of the repacked
# .app.tar.gz. A second copy of these resolution rules is exactly how a release
# ends up with a key that is "set" but emits no .sig (the v0.11.0 failure).
#
# THE KEY GOES THROUGH THE ENVIRONMENT, NEVER ARGV. `ps -eo command` is
# world-readable, and the legacy TAURI_SIGNING_PRIVATE_KEY form holds the key
# CONTENTS, so `--private-key <key material>` would publish the updater private
# key to every local process for the duration of the sign; a passworded key would
# put its password there too. This is the same rule notarytool_run follows for
# APPLE_PASSWORD, and it matters more here because this file SHIPS to the public
# mirror (release_signing.sh, which used to hold the only such invocation, does
# not), so the line doubles as published example code.
#
# The fix is a shell PREFIX assignment, which bash applies to the child's
# environment without ever putting it in an argument vector. Measured on this
# machine: with the key as a flag, `ps -eo command` shows it; as a prefix
# assignment, it appears nowhere. (`env VAR=value cmd` does put the value in
# env's own argv, though env execs rather than forks so the window is brief; a
# prefix assignment has no window and no extra process, so prefer it.)
#
# The assignment sits in front of `env -u <the other key var>`, so the
# environment the signer sees is fully determined here (exactly one of the two
# key vars present) rather than partly inherited, and env's own argv carries only
# a variable NAME. The signer reads all three vars itself (see
# `cargo tauri signer sign --help`), so nothing is lost by dropping the flags.
#
# A resolved value naming an existing FILE goes in TAURI_SIGNING_PRIVATE_KEY_PATH;
# anything else is the key CONTENTS. The two branches are spelled out rather than
# built from a variable because a prefix assignment cannot take a dynamic name
# without `eval`, and an `eval` holding key material is a worse trade than four
# duplicated lines. The password defaults to EMPTY rather than being left unset:
# a no-password key is still signed with an empty password, and setting it
# explicitly keeps the path robust if Tauri ever stops defaulting it.
#
# Silent on success; on failure returns non-zero after printing the signer's own
# output to stderr, so the caller can attribute the failure without re-running it.
tauri_signer_sign_file() {
    local file="$1" key pw out rc=0
    if ! key="$(tauri_signing_key_resolved)"; then
        echo "ERROR: no updater key configured (set TAURI_SIGNING_PRIVATE_KEY_PATH to the key file, or TAURI_SIGNING_PRIVATE_KEY to its contents)." >&2
        return 1
    fi
    pw="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

    if [ -f "$key" ]; then
        out="$(TAURI_SIGNING_PRIVATE_KEY_PATH="$key" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$pw" \
               env -u TAURI_SIGNING_PRIVATE_KEY \
               cargo tauri signer sign "$file" 2>&1)" || rc=$?
    else
        out="$(TAURI_SIGNING_PRIVATE_KEY="$key" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$pw" \
               env -u TAURI_SIGNING_PRIVATE_KEY_PATH \
               cargo tauri signer sign "$file" 2>&1)" || rc=$?
    fi

    if [ "$rc" != "0" ]; then
        printf '%s\n' "$out" >&2
        return 1
    fi
    return 0
}
