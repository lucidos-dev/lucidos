#!/usr/bin/env bash
# Tests for scripts/lib/rc_channel.sh and scripts/publish-rc-channel.sh. Offline:
# a temp staging, a temp workspace and a fake HOME. The core assertion is that
# a publish writes only the rc folder, and leaves the staging and HOME as they
# were. Run: ./scripts/lib/rc_channel_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/rc_channel.sh
source "$SCRIPT_DIR/rc_channel.sh"
PUBLISH="$SCRIPT_DIR/../publish-rc-channel.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
VERSION="9.9.1"
PORT="5199"

# new_case: a fresh staging, workspace and HOME under $ROOT/<n>.
CASE=0
new_case() {
    CASE=$((CASE+1))
    STAGING="$ROOT/$CASE/staging"
    WS="$ROOT/$CASE/ws"
    FAKE_HOME="$ROOT/$CASE/home"
    mkdir -p "$STAGING" "$WS/.lucidos" "$WS/data/artifacts" "$FAKE_HOME/.lucidos"
    printf 'fake dmg\n' > "$STAGING/Lucidos_${VERSION}_aarch64.dmg"
    printf 'fake updater bundle\n' > "$STAGING/Lucidos.app.tar.gz"
    printf 'ZmFrZSBzaWduYXR1cmU=\n' > "$STAGING/Lucidos.app.tar.gz.sig"
    RELEASE_STAGING_PLATFORM_KEY="darwin-aarch64" release_staging_write_manifest \
        "$STAGING" "$VERSION" deadbeef \
        "Lucidos_${VERSION}_aarch64.dmg" Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
    printf 'API_PORT=%s\nVITE_PORT=%s\nPROTO=http\n' "$PORT" "$PORT" > "$WS/.lucidos/ports"
    printf 'keep\n' > "$FAKE_HOME/.lucidos/updates.toml"
}

# snapshot <dir>: every file with its checksum, so any change shows.
snapshot() {
    (cd "$1" && find . -type f -print0 | sort -z | xargs -0 shasum)
}

# json_field <file> <python expression over m>: print the value.
json_field() {
    python3 -c 'import json,sys; m=json.load(open(sys.argv[1])); print(eval(sys.argv[2]))' "$1" "$2"
}

RC_URL="http://127.0.0.1:$PORT/api/v1/data/artifacts/releases/rc"

echo "test: the script publishes a staged rc into the workspace's rc folder"
new_case
STAGING_BEFORE="$(snapshot "$STAGING")"
HOME_BEFORE="$(snapshot "$FAKE_HOME")"
WS_BEFORE="$(cd "$WS" && find . -type f | sort)"
if out="$(HOME="$FAKE_HOME" "$PUBLISH" "$VERSION" --workspace "$WS" --staging-dir "$STAGING" 2>&1)"; then
    pass "publish succeeds"
else
    fail "publish failed: $out"
fi
RC="$WS/data/artifacts/releases/rc"
files="$(cd "$RC" && find . -type f | LC_ALL=C sort | tr '\n' ' ')"
want="./.gitignore ./Lucidos.app.tar.gz ./Lucidos.app.tar.gz.sig ./latest.json "
if [ "$files" = "$want" ]; then pass "rc folder holds exactly the four files"; else fail "rc files: '$files'"; fi
if [ "$(cat "$RC/.gitignore")" = "*" ]; then pass ".gitignore ignores everything"; else fail ".gitignore is '$(cat "$RC/.gitignore")'"; fi
if cmp -s "$STAGING/Lucidos.app.tar.gz" "$RC/Lucidos.app.tar.gz"; then pass "bundle copied byte for byte"; else fail "bundle differs"; fi
if [ "$(json_field "$RC/latest.json" 'm["version"]')" = "$VERSION" ]; then pass "latest.json version"; else fail "latest.json version wrong"; fi
if [ "$(json_field "$RC/latest.json" 'list(m["platforms"])')" = "['darwin-aarch64']" ]; then
    pass "latest.json platform key comes from the manifest"
else
    fail "platforms: $(json_field "$RC/latest.json" 'list(m["platforms"])')"
fi
if [ "$(json_field "$RC/latest.json" 'm["platforms"]["darwin-aarch64"]["url"]')" = "$RC_URL/Lucidos.app.tar.gz" ]; then
    pass "latest.json points at the loopback bundle"
else
    fail "url: $(json_field "$RC/latest.json" 'm["platforms"]["darwin-aarch64"]["url"]')"
fi
if [ "$(json_field "$RC/latest.json" 'm["platforms"]["darwin-aarch64"]["signature"]')" = "ZmFrZSBzaWduYXR1cmU=" ]; then
    pass "latest.json carries the staged signature"
else
    fail "signature wrong"
fi
case "$out" in
    *"lucidos update-address set $RC_URL/latest.json"*) pass "prints the update-address command" ;;
    *) fail "output lacks the set command: $out" ;;
esac
if [ "$(snapshot "$STAGING")" = "$STAGING_BEFORE" ]; then pass "staging untouched"; else fail "staging changed"; fi
if [ "$(snapshot "$FAKE_HOME")" = "$HOME_BEFORE" ]; then pass "HOME untouched"; else fail "HOME changed"; fi
WS_AFTER="$(cd "$WS" && find . -type f ! -path './data/artifacts/releases/rc/*' | sort)"
if [ "$WS_AFTER" = "$WS_BEFORE" ]; then pass "nothing outside the rc folder in the workspace"; else fail "workspace gained: $WS_AFTER"; fi

echo "test: a second publish replaces the first"
printf 'newer bundle\n' > "$STAGING/Lucidos.app.tar.gz"
RELEASE_STAGING_PLATFORM_KEY="darwin-aarch64" release_staging_write_manifest \
    "$STAGING" "$VERSION" deadbeef \
    "Lucidos_${VERSION}_aarch64.dmg" Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
if HOME="$FAKE_HOME" "$PUBLISH" "$VERSION" --workspace "$WS" --staging-dir "$STAGING" >/dev/null 2>&1 \
        && cmp -s "$STAGING/Lucidos.app.tar.gz" "$RC/Lucidos.app.tar.gz"; then
    pass "the rc folder holds the newer bundle"
else
    fail "second publish did not replace the bundle"
fi

echo "test: LUCIDOS_WORKSPACE is the default workspace"
new_case
if HOME="$FAKE_HOME" LUCIDOS_WORKSPACE="$WS" "$PUBLISH" "$VERSION" --staging-dir "$STAGING" >/dev/null 2>&1 \
        && [ -f "$WS/data/artifacts/releases/rc/latest.json" ]; then
    pass "published into \$LUCIDOS_WORKSPACE"
else
    fail "LUCIDOS_WORKSPACE not used"
fi

# refuses <label> <expected-stderr-fragment>: run the publish, expect a refusal
# that names the cause and writes no rc folder.
refuses() {
    local label="$1" fragment="$2" err
    if err="$(HOME="$FAKE_HOME" "$PUBLISH" "$VERSION" --workspace "$WS" --staging-dir "$STAGING" 2>&1)"; then
        fail "$label: publish succeeded"
        return
    fi
    case "$err" in
        *"$fragment"*) pass "$label: refused" ;;
        *) fail "$label: wrong message: $err" ;;
    esac
    if [ -e "$WS/data/artifacts/releases/rc" ]; then fail "$label: rc folder written"; fi
}

echo "test: refusals write nothing"
new_case
printf 'API_PORT=%s\nPROTO=https\n' "$PORT" > "$WS/.lucidos/ports"
refuses "https engine" "not plain http"

new_case
printf 'API_PORT=%s\n' "$PORT" > "$WS/.lucidos/ports"
refuses "ports file without PROTO" "not plain http"

new_case
rm "$WS/.lucidos/ports"
refuses "no ports file" "no ports file"

new_case
printf 'tampered\n' > "$STAGING/Lucidos.app.tar.gz"
refuses "tampered staging" "checksum mismatch"

new_case
VERSION_SAVED="$VERSION"
VERSION="9.9.2"
refuses "version mismatch" "holds v$VERSION_SAVED"
VERSION="$VERSION_SAVED"

echo "test: rc_channel_url reads the port and protocol"
new_case
if [ "$(rc_channel_url "$WS")" = "$RC_URL/latest.json" ]; then pass "loopback URL"; else fail "url: $(rc_channel_url "$WS")"; fi

echo
echo "rc_channel: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
