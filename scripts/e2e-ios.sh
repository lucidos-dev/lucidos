#!/bin/bash
# Boot an iOS Simulator with Safari pointed at the e2e workspace.
# Requires Xcode (not just Command Line Tools).
#
# Usage:
#   ./scripts/e2e-ios.sh                    # Boot simulator + open Safari
#   ./scripts/e2e-ios.sh --screenshot       # Take a screenshot after loading
#   ./scripts/e2e-ios.sh --device "iPhone 17 Pro"  # Specific device
#   ./scripts/e2e-ios.sh --pwa              # Add to Home Screen (web clip)
#   ./scripts/e2e-ios.sh --kill             # Shut down the simulator
#
# The script:
# 1. Ensures the e2e workspace is running
# 2. Boots an iOS Simulator (iPhone 17 Pro by default)
# 3. Opens Safari to the CognOS URL
# 4. Optionally takes a screenshot or installs as PWA
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

source "$SCRIPT_DIR/lib/e2e.sh"

# ── Defaults ──
DEVICE_NAME="iPhone 17 Pro"
TAKE_SCREENSHOT=""
INSTALL_PWA=""
KILL_SIM=""
SCREENSHOT_DIR="$PROJECT_DIR/crates/cognos-app/e2e/screenshots"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --device) DEVICE_NAME="$2"; shift 2 ;;
        --screenshot) TAKE_SCREENSHOT=1; shift ;;
        --pwa) INSTALL_PWA=1; shift ;;
        --kill) KILL_SIM=1; shift ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# ── Check Xcode ──
check_xcode() {
    if ! command -v xcrun &>/dev/null || ! xcrun simctl list devices &>/dev/null 2>&1; then
        echo "ERROR: Xcode is required for iOS Simulator testing."
        echo ""
        echo "Install Xcode from the Mac App Store:"
        echo "  1. Open App Store and search for 'Xcode'"
        echo "  2. Install (requires ~35GB disk space)"
        echo "  3. Run: sudo xcode-select -s /Applications/Xcode.app"
        echo "  4. Run: xcodebuild -license accept"
        echo "  5. Re-run this script"
        echo ""
        echo "For automated WebKit testing without Xcode, use:"
        echo "  ./scripts/e2e-browser.sh --webkit"
        exit 1
    fi
}

# ── Kill simulator ──
if [ -n "$KILL_SIM" ]; then
    check_xcode
    echo "Shutting down iOS Simulator..."
    xcrun simctl shutdown all 2>/dev/null || true
    killall "Simulator" 2>/dev/null || true
    echo "Done"
    exit 0
fi

check_xcode

# Returns "udid|state" for the best matching device.
# Priority: already booted > match by name > any iPhone.
find_device() {
    local device_name="$1"
    xcrun simctl list devices -j 2>/dev/null | \
        python3 -c "
import json, sys

name = sys.argv[1]
data = json.load(sys.stdin)

# Already booted — use it regardless of name
for devices in data.get('devices', {}).values():
    for d in devices:
        if d.get('state') == 'Booted':
            print(d['udid'] + '|Booted')
            sys.exit(0)

# Match by name (prefer newest runtime)
for runtime in sorted(data.get('devices', {}).keys(), reverse=True):
    for d in data['devices'][runtime]:
        if d.get('name') == name and d.get('isAvailable', False):
            print(d['udid'] + '|' + d.get('state', 'Shutdown'))
            sys.exit(0)

# Fallback: any available iPhone
for runtime in sorted(data.get('devices', {}).keys(), reverse=True):
    for d in data['devices'][runtime]:
        if 'iPhone' in d.get('name', '') and d.get('isAvailable', False):
            print(d['udid'] + '|' + d.get('state', 'Shutdown'))
            sys.exit(0)

sys.exit(1)
" "$device_name" 2>/dev/null
}

ensure_workspace_running

ENGINE_URL="${PROTO}://localhost:${VITE_PORT}"
echo "CognOS URL: $ENGINE_URL"

DEVICE_RESULT=$(find_device "$DEVICE_NAME")
if [ -z "$DEVICE_RESULT" ]; then
    echo "ERROR: No iOS Simulator device found matching '$DEVICE_NAME'"
    echo ""
    echo "Available devices:"
    xcrun simctl list devices available | grep -E "iPhone|iPad" | head -10
    echo ""
    echo "Install more runtimes via: Xcode > Settings > Platforms"
    exit 1
fi

DEVICE_UDID="${DEVICE_RESULT%%|*}"
DEVICE_STATE="${DEVICE_RESULT##*|}"

if [ "$DEVICE_STATE" != "Booted" ]; then
    echo "Booting simulator: $DEVICE_NAME ($DEVICE_UDID)..."
    xcrun simctl boot "$DEVICE_UDID"
    # Open Simulator.app to show the window
    open -a Simulator
    # Wait for boot
    echo "Waiting for simulator to boot..."
    xcrun simctl bootstatus "$DEVICE_UDID" -b 2>/dev/null || sleep 5
else
    echo "Simulator already booted: $DEVICE_NAME ($DEVICE_UDID)"
    open -a Simulator
fi

# ── Trust mkcert root CA (if using HTTPS) ──
if [ "$PROTO" = "https" ] && command -v mkcert &>/dev/null; then
    MKCERT_CA="$(mkcert -CAROOT)/rootCA.pem"
    if [ -f "$MKCERT_CA" ]; then
        echo "Installing mkcert root CA into simulator trust store..."
        xcrun simctl keychain "$DEVICE_UDID" add-root-cert "$MKCERT_CA" 2>/dev/null || true
    fi
fi

# ── Open Safari ──
# Launch Safari first — openurl immediately after boot times out (POSIX code 60)
# because Safari isn't ready to handle URL schemes yet.
xcrun simctl launch "$DEVICE_UDID" com.apple.mobilesafari 2>/dev/null || true
sleep 3
echo "Opening Safari to $ENGINE_URL ..."
xcrun simctl openurl "$DEVICE_UDID" "$ENGINE_URL"

# ── Install as PWA (web clip) ──
if [ -n "$INSTALL_PWA" ]; then
    echo "To add as PWA:"
    echo "  1. In Safari, tap the Share button (box with arrow)"
    echo "  2. Tap 'Add to Home Screen'"
    echo "  3. Tap 'Add'"
    echo ""
    echo "Note: Web clip profiles can be installed via:"
    echo "  xcrun simctl install $DEVICE_UDID <path-to-webclip.mobileconfig>"
fi

# ── Screenshot ──
if [ -n "$TAKE_SCREENSHOT" ]; then
    mkdir -p "$SCREENSHOT_DIR"
    TIMESTAMP=$(date +%Y%m%d-%H%M%S)
    SCREENSHOT_PATH="$SCREENSHOT_DIR/ios-sim-${TIMESTAMP}.png"

    echo "Waiting 5s for page to load..."
    sleep 5

    xcrun simctl io "$DEVICE_UDID" screenshot "$SCREENSHOT_PATH"
    echo "Screenshot saved: $SCREENSHOT_PATH"
fi

echo ""
echo "iOS Simulator is running. To take screenshots later:"
echo "  xcrun simctl io $DEVICE_UDID screenshot output.png"
echo ""
echo "To shut down:"
echo "  ./scripts/e2e-ios.sh --kill"
