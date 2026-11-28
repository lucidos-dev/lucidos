#!/usr/bin/env bash
# Exercise real disk images on macOS; never signal or modify a live engine.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/dmg_volume.sh
source "$SCRIPT_DIR/dmg_volume.sh"
WORK="$(mktemp -d)"
cleanup_test() {
    hdiutil detach "$WORK/mounted" -force >/dev/null 2>&1 || true
    rm -rf "$WORK"
}
trap cleanup_test EXIT
export PYTHONDONTWRITEBYTECODE=1
python3 - "$SCRIPT_DIR" "$WORK" <<'PY'
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from dmg_layout import layout_bytes, read_layout
work = Path(sys.argv[2])
first = layout_bytes()
assert first == layout_bytes(), 'Layout bytes are nondeterministic'
(work / 'layout').write_bytes(first)
records = read_layout(work / 'layout')
assert len(records) == 5
assert records['.', 'bwsp'] == {
    'ShowStatusBar': False, 'ShowToolbar': False, 'ShowTabView': False,
    'ContainerShowSidebar': False, 'ShowSidebar': False,
    'WindowBounds': '{{10, 980}, {660, 400}}',
}
assert records['.', 'icvp'] == {
    'backgroundColorRed': 1.0, 'backgroundColorGreen': 1.0,
    'backgroundColorBlue': 1.0, 'showIconPreview': True, 'textSize': 16.0,
    'backgroundType': 0, 'gridOffsetX': 0.0, 'gridOffsetY': 0.0,
    'showItemInfo': False, 'viewOptionsVersion': 1, 'arrangeBy': 'none',
    'labelOnBottom': True, 'iconSize': 128.0, 'gridSpacing': 100.0,
}
assert records['.', 'vSrn'] == 1
assert records['Applications', 'Iloc'] == (480, 170, b'\xff' * 6 + b'\0\0')
assert records['Lucidos.app', 'Iloc'] == (180, 170, b'\xff' * 6 + b'\0\0')
print('ok: deterministic layout and all five shipped records')
PY
[ "$(dmg_volume_path /bundle 0.77.0 arm64)" = /bundle/Lucidos_0.77.0_aarch64.dmg ]
[ "$(dmg_volume_path /bundle 0.77.0 x86_64)" = /bundle/Lucidos_0.77.0_x64.dmg ]
if dmg_volume_path /bundle 0.77.0 unsupported; then
    echo 'FAIL: accepted unsupported architecture' >&2
    false
fi
dmg_volume_prepare_directory "$WORK/output"
touch "$WORK/output/old.dmg" "$WORK/output/old.rw.dmg" "$WORK/output/keep.txt"
dmg_volume_prepare_directory "$WORK/output"
[ ! -e "$WORK/output/old.dmg" ]
[ ! -e "$WORK/output/old.rw.dmg" ]
[ -f "$WORK/output/keep.txt" ]
if [ "$(uname -s)" != Darwin ]; then
    echo 'SKIP: disk image integration requires macOS'
else
    APP="$WORK/Fixture.app"
    mkdir -p "$APP/Contents/Resources" "$WORK/mounted"
    cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>CFBundleShortVersionString</key><string>0.77.0</string><key>CFBundleExecutable</key><string>fixture</string><key>CFBundleIconFile</key><string>fixture</string></dict></plist>
PLIST
    printf 'fixture icon\n' > "$APP/Contents/Resources/fixture.icns"
    printf 'payload\n' > "$APP/Contents/payload"
    DMG="$WORK/Lucidos_0.77.0_aarch64.dmg"
    # The binary, rather than the shell's architecture, determines the filename.
    lipo() { [ "$1" = -archs ] && [ "$2" = "$APP/Contents/MacOS/fixture" ] && echo arm64; }
    [ "$(dmg_volume_app_path "$WORK" "$APP")" = "$DMG" ]
    lipo() { echo x86_64; }
    [ "$(dmg_volume_app_path "$WORK" "$APP")" = "$WORK/Lucidos_0.77.0_x64.dmg" ]
    unset -f lipo
    dmg_volume_assemble "$DMG" "$APP"
    [ ! -e "$(release_dmg_rw_path "$DMG")" ]
    [ ! -e "$(release_dmg_zlib_path "$DMG")" ]
    hdiutil imageinfo -plist "$DMG" > "$WORK/image.plist"
    python3 - "$WORK/image.plist" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'rb') as source:
    info = plistlib.load(source)
assert info['Format'] == 'UDZO', info
assert info['partitions']['partition-scheme'] == 'GUID', info
assert any('HFS+' in p.get('partition-filesystems', {})
           for p in info['partitions']['partitions']), info
PY
    hdiutil attach "$DMG" -nobrowse -noautoopen -mountpoint "$WORK/mounted" >/dev/null
    cmp "$WORK/layout" "$WORK/mounted/.DS_Store"
    cmp "$APP/Contents/payload" "$WORK/mounted/Lucidos.app/Contents/payload"
    cmp "$APP/Contents/Resources/fixture.icns" "$WORK/mounted/.VolumeIcon.icns"
    [ "$(readlink "$WORK/mounted/Applications")" = /Applications ]
    stat -f '%Sf' "$WORK/mounted/.VolumeIcon.icns" | grep -q hidden
    [ "$(xattr -px com.apple.FinderInfo "$WORK/mounted" | tr -d ' \n')" = 0000000000000000040000000000000000000000000000000000000000000000 ]
    hdiutil detach "$WORK/mounted" >/dev/null
    # A failed assembly must preserve the existing final image and remove leftovers.
    cp "$DMG" "$WORK/original.dmg"
    if dmg_volume_assemble "$DMG" "$WORK/missing.app"; then
        echo 'FAIL: missing app succeeded' >&2
        false
    fi
    cmp "$DMG" "$WORK/original.dmg"
    [ ! -e "$(release_dmg_rw_path "$DMG")" ]
    [ ! -e "$(release_dmg_zlib_path "$DMG")" ]
    # Fail after mounting to exercise detach and intermediate removal.
    hdiutil() {
        if [ "$1" = attach ]; then
            printf '%s' "$6" > "$WORK/assembly-mountpoint"
        fi
        command hdiutil "$@"
    }
    xattr() { return 1; }
    if dmg_volume_assemble "$DMG" "$APP"; then
        echo 'FAIL: failed Finder flag write succeeded' >&2
        false
    fi
    unset -f xattr hdiutil
    [ ! -d "$(cat "$WORK/assembly-mountpoint")" ]
    cmp "$DMG" "$WORK/original.dmg"
    [ ! -e "$(release_dmg_rw_path "$DMG")" ]
    [ ! -e "$(release_dmg_zlib_path "$DMG")" ]
    hdiutil() {
        if [ "$1" = attach ]; then
            printf '%s' "$6" > "$WORK/assembly-mountpoint"
            return 1
        fi
        command hdiutil "$@"
    }
    if dmg_volume_assemble "$DMG" "$APP"; then
        echo 'FAIL: failed attach succeeded' >&2
        false
    fi
    unset -f hdiutil
    [ ! -d "$(cat "$WORK/assembly-mountpoint")" ]
    cmp "$DMG" "$WORK/original.dmg"
    # An attach can report failure after mounting; cleanup must still detach.
    hdiutil() {
        command hdiutil "$@" || return 1
        if [ "$1" = attach ]; then
            printf '%s' "$6" > "$WORK/assembly-mountpoint"
            return 1
        fi
    }
    if dmg_volume_assemble "$DMG" "$APP"; then
        echo 'FAIL: partially successful attach succeeded' >&2
        false
    fi
    unset -f hdiutil
    assembly_mount="$(cat "$WORK/assembly-mountpoint")"
    if python3 -c 'import os, sys; sys.exit(not os.path.ismount(sys.argv[1]))' "$assembly_mount"; then
        command hdiutil detach "$assembly_mount" -force >/dev/null
        rm -rf "$(dirname "$assembly_mount")"
        echo 'FAIL: partially successful attach left a mounted volume' >&2
        false
    fi
    [ ! -d "$assembly_mount" ]
    cmp "$DMG" "$WORK/original.dmg"
    echo 'ok: mounted volume, Finder flags, payload, format and failure cleanup'
fi
# The bundler must never enter its desktop-driven disk-image path.
BUILD="$SCRIPT_DIR/../build-dmg.sh"
grep -q 'TAURI_BUILD_ARGS=(tauri build --bundles "app" ' "$BUILD"
if grep -nE 'osascript|tell application "Finder"' "$BUILD" \
    "$SCRIPT_DIR/dmg_volume.sh" "$SCRIPT_DIR/dmg_layout.py"; then
    echo 'FAIL: desktop automation in disk-image assembly' >&2
    false
fi
echo 'ok: app-only bundling and no desktop automation'
