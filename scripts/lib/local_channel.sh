#!/usr/bin/env bash
# The gateway is the authority for Local's directory and serving origin.
# shellcheck source=scripts/lib/release_staging.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release_staging.sh"
# shellcheck source=scripts/lib/release_upload.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release_upload.sh"

local_channel_publish() (
    local staging="$1" version="$2" port="${3:-}" status dir tarball name digest tmp=""
    local cli_args=(update-channel show)
    if [ -n "$port" ]; then cli_args+=(--port "$port"); fi
    status="$(lucidos "${cli_args[@]}")" || return 1
    dir="$(printf '%s\n' "$status" | sed -n 's/^Local staging directory: //p')"
    case "$dir" in
        /*) ;;
        *) echo 'ERROR: gateway did not report an absolute Local staging directory. Update Lucidos first.' >&2; return 1 ;;
    esac
    release_staging_verify "$staging" || return 1
    mkdir -p "$dir" || return 1
    tmp="$(mktemp -d "$dir/.publish.XXXXXX")" || return 1
    trap 'rm -rf "$tmp"' EXIT
    tarball="$(release_staging_copy_updater "$staging" "$tmp" "$version")" || return 1
    digest="$(release_staging_sha256 "$tarball")" || return 1
    name="Lucidos-$digest.app.tar.gz"
    if ! mv "$tarball" "$tmp/$name" \
        || ! release_upload_write_latest_json "$tmp/latest.json" "$version" darwin-aarch64 \
            "$name" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$tarball.sig" \
        || ! mv "$tmp/$name" "$dir/$name" \
        || ! mv "$tmp/latest.json" "$dir/latest.json"; then
        rm -rf "$tmp"
        return 1
    fi
)
