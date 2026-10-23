#!/usr/bin/env bash
# rc_channel.sh: publish a staged release candidate to the local rc channel.
#
# The rc channel is a folder in a dev workspace, data/artifacts/releases/rc/.
# Its engine serves the folder on loopback, so a packaged app whose update
# address points at the rc latest.json updates to the staged build. The bundle
# keeps its real signature: the app checks it with the baked key as usual.
#
# The only writes go into that rc folder. Nothing here touches the release
# staging, ~/.lucidos or the app's own data. rc_channel_test.sh asserts it.

# shellcheck source=scripts/lib/release_staging.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release_staging.sh"
# shellcheck source=scripts/lib/release_upload.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release_upload.sh"

RC_CHANNEL_SUBDIR="data/artifacts/releases/rc"

# rc_channel_ports_value <ports-file> <key>: print the value of KEY=... or nothing.
rc_channel_ports_value() {
    local file="$1" key="$2" line
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
            "$key="*) printf '%s' "${line#*=}"; return 0 ;;
        esac
    done < "$file"
}

# rc_channel_url <workspace>: print the loopback latest.json URL of the channel.
# Refuses a workspace without a ports file, and an engine that serves https,
# because the update address allows plain http only to loopback and a
# self-signed loopback certificate fails the updater's TLS check.
rc_channel_url() {
    local workspace="$1" ports="$1/.lucidos/ports" port proto
    [ -f "$ports" ] || {
        echo "ERROR: no ports file at $ports." >&2
        echo "       Start the workspace's engine first, so it records its port." >&2
        return 1
    }
    port="$(rc_channel_ports_value "$ports" API_PORT)"
    proto="$(rc_channel_ports_value "$ports" PROTO)"
    case "$port" in
        ''|*[!0-9]*)
            echo "ERROR: $ports has no numeric API_PORT ('$port')." >&2
            return 1 ;;
    esac
    if [ "$proto" != "http" ]; then
        echo "ERROR: the engine for $workspace serves '${proto:-https}', not plain http." >&2
        echo "       The packaged updater cannot trust a loopback certificate, so the rc" >&2
        echo "       channel needs a plain-http engine. Use a workspace without TLS." >&2
        return 1
    fi
    printf 'http://127.0.0.1:%s/api/v1/%s/latest.json' "$port" "$RC_CHANNEL_SUBDIR"
}

# rc_channel_publish <staging-dir> <workspace> <version>: verify the staging,
# then copy its updater bundle and signature into the rc folder and write a
# latest.json that points at them. latest.json goes last, so the channel never
# advertises a bundle that is not there yet.
rc_channel_publish() {
    local staging="$1" workspace="$2" version="$3"
    local staged_version platform_key url rc tarball sig name

    [ -d "$staging" ] || { echo "ERROR: no release staging at $staging" >&2; return 1; }
    [ -d "$workspace" ] || { echo "ERROR: no workspace at $workspace" >&2; return 1; }
    release_staging_verify "$staging" || return 1
    staged_version="$(release_staging_manifest_field "$staging" version)" || return 1
    if [ "$staged_version" != "$version" ]; then
        echo "ERROR: $staging holds v$staged_version, not v$version." >&2
        return 1
    fi
    platform_key="$(release_staging_platform_key "$staging")" || return 1
    url="$(rc_channel_url "$workspace")" || return 1

    tarball=""
    for name in "$staging"/*.app.tar.gz; do
        [ -f "$name" ] || continue
        [ -z "$tarball" ] || { echo "ERROR: more than one .app.tar.gz in $staging" >&2; return 1; }
        tarball="$name"
    done
    [ -n "$tarball" ] || { echo "ERROR: no .app.tar.gz in $staging" >&2; return 1; }
    sig="$tarball.sig"
    [ -f "$sig" ] || { echo "ERROR: no signature beside $tarball" >&2; return 1; }
    name="$(basename "$tarball")"

    rc="$workspace/$RC_CHANNEL_SUBDIR"
    mkdir -p "$rc" || return 1
    # The bundle is about 100 MB. Keep it out of the workspace history.
    printf '*\n' > "$rc/.gitignore" || return 1
    rm -f "$rc/latest.json"
    cp "$tarball" "$rc/$name" || return 1
    cp "$sig" "$rc/$name.sig" || return 1
    release_upload_write_latest_json "$rc/latest.json.tmp" "$version" "$platform_key" \
        "${url%/latest.json}/$name" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$rc/$name.sig" \
        || { rm -f "$rc/latest.json.tmp"; return 1; }
    mv "$rc/latest.json.tmp" "$rc/latest.json" || return 1
    printf '%s' "$url"
}
