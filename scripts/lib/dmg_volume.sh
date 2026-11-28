#!/usr/bin/env bash
# Assemble the final app and Finder layout without a desktop automation service.

DMG_VOLUME_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/release_dmg.sh
source "$DMG_VOLUME_LIB_DIR/release_dmg.sh"

dmg_volume_path() {
    local directory="$1" version="$2" arch="$3"
    case "$arch" in
        arm64|aarch64) arch=aarch64 ;;
        x86_64|x64) arch=x64 ;;
        *) echo "ERROR: unsupported DMG architecture: $arch" >&2; return 1 ;;
    esac
    printf '%s/Lucidos_%s_%s.dmg' "$directory" "$version" "$arch"
}

# Name from the app's stamped version so the caller can detect stamping drift.
dmg_volume_app_path() {
    local version executable arch
    version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$2/Contents/Info.plist")" \
        || return 1
    [ -n "$version" ] || return 1
    executable="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$2/Contents/Info.plist")" \
        || return 1
    arch="$(lipo -archs "$2/Contents/MacOS/$executable")" || return 1
    dmg_volume_path "$1" "$version" "$arch"
}

# A new build must not expose previous builds to submission adoption.
dmg_volume_prepare_directory() {
    mkdir -p "$1" || return 1
    /usr/bin/find "$1" -maxdepth 1 -type f -name '*.dmg' -delete
}

# Subshell traps clean up on failure, including when the caller disables errexit.
dmg_volume_assemble() (
    set -euo pipefail
    local dmg="$1" app="$2" rw out work mnt icon
    rw="$(release_dmg_rw_path "$dmg")" || return 1
    out="$(release_dmg_zlib_path "$dmg")" || return 1
    work="$(mktemp -d)" || return 1
    mnt="$work/mount"
    # shellcheck disable=SC2329 # Invoked through the EXIT trap.
    cleanup() {
        local status=$? is_mounted
        trap - EXIT
        # Attach can fail after mounting, so inspect the filesystem itself.
        is_mounted="$(python3 -c 'import os, sys; print(int(os.path.ismount(sys.argv[1])))' "$mnt")" || {
            echo "ERROR: could not inspect $mnt; preserving mount directory and images" >&2
            return 1
        }
        if [ "$is_mounted" = 1 ]; then
            if ! hdiutil detach "$mnt" -force >/dev/null; then
                echo "ERROR: could not detach $mnt; preserving mount directory" >&2
                rm -f "$rw" "$out"
                return 1
            fi
        fi
        rm -f "$rw" "$out"
        rm -rf "$work"
        return "$status"
    }
    trap cleanup EXIT
    trap 'return 130' INT
    trap 'return 143' TERM
    mkdir -p "$(dirname "$dmg")" "$work/payload" "$mnt" || return 1
    rm -f "$rw" "$out" || return 1
    ditto "$app" "$work/payload/Lucidos.app" || return 1
    ln -s /Applications "$work/payload/Applications" || return 1
    icon="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$app/Contents/Info.plist")" || return 1
    case "$icon" in *.icns) ;; *) icon="$icon.icns" ;; esac
    cp "$app/Contents/Resources/$icon" "$work/payload/.VolumeIcon.icns" || return 1
    python3 "$DMG_VOLUME_LIB_DIR/dmg_layout.py" "$work/payload/.DS_Store" || return 1
    hdiutil create -volname Lucidos -fs HFS+ -layout GPTSPUD \
        -srcfolder "$work/payload" -format UDRW -ov "$rw" >/dev/null || return 1
    hdiutil attach "$rw" -nobrowse -noautoopen -mountpoint "$mnt" >/dev/null || return 1
    xattr -wx com.apple.FinderInfo 0000000000000000040000000000000000000000000000000000000000000000 "$mnt" || return 1
    chflags hidden "$mnt/.VolumeIcon.icns" || return 1
    hdiutil detach "$mnt" -force >/dev/null || return 1
    hdiutil convert "$rw" -format UDZO -imagekey zlib-level=9 -o "$out" >/dev/null || return 1
    mv -f "$out" "$dmg" || return 1
)
