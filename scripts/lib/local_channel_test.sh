#!/usr/bin/env bash
# Offline Local publisher tests: all gateway reads use a fake CLI.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/test_skip.sh
source "$SCRIPT_DIR/test_skip.sh"
test_skip_unless_present "$SCRIPT_DIR/../.." scripts/lib/update_channel_feed.sh
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/release_staging.sh"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
mkdir -p "$ROOT/bin" "$ROOT/staging" "$ROOT/gateway chosen/local"
export LOCAL_TEST_DIR="$ROOT/gateway chosen/local" LOCAL_TEST_CALLS="$ROOT/calls"
cat > "$ROOT/bin/lucidos" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$LOCAL_TEST_CALLS"
[ "$*" = 'update-channel show --port 5999' ] || exit 1
printf 'Update channel: stable\nLocal staging directory: %s\n' "$LOCAL_TEST_DIR"
SH
chmod +x "$ROOT/bin/lucidos"
export PATH="$ROOT/bin:$PATH"
printf bundle > "$ROOT/staging/Lucidos.app.tar.gz"
printf signature > "$ROOT/staging/Lucidos.app.tar.gz.sig"
printf dmg > "$ROOT/staging/Lucidos.dmg"
RELEASE_STAGING_PLATFORM_KEY=darwin-aarch64 release_staging_write_manifest \
    "$ROOT/staging" 9.9.1 deadbeef Lucidos.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
PUBLISH="$SCRIPT_DIR/../publish-local-channel.sh"
"$PUBLISH" 9.9.1 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/out"
python3 - "$LOCAL_TEST_DIR" "$ROOT/staging/manifest.json" <<'PY'
import datetime, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
feed = json.loads((root / 'latest.json').read_text())
assert feed['version'] == '9.9.1'
expected_date = datetime.datetime.fromtimestamp(pathlib.Path(sys.argv[2]).stat().st_mtime, datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
assert feed['pub_date'] == expected_date
assert list(feed['platforms']) == ['darwin-aarch64']
entry = feed['platforms']['darwin-aarch64']
assert '/' not in entry['url'], 'stored Local feed must carry no origin or port'
assert (root / entry['url']).read_bytes() == b'bundle'
assert entry['signature'] == 'signature'
PY
# Publishing another candidate cannot overwrite the bundle an in-flight reader uses.
cp "$LOCAL_TEST_DIR/latest.json" "$ROOT/first.json"
printf newer > "$ROOT/staging/Lucidos.app.tar.gz"
RELEASE_STAGING_PLATFORM_KEY=darwin-aarch64 release_staging_write_manifest \
    "$ROOT/staging" 9.9.2 deadbeef Lucidos.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
"$PUBLISH" 9.9.2 --port 5999 --staging-dir "$ROOT/staging" > /dev/null
python3 - "$LOCAL_TEST_DIR" "$ROOT/first.json" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
a = json.loads(pathlib.Path(sys.argv[2]).read_text())['platforms']['darwin-aarch64']['url']
b = json.loads((root / 'latest.json').read_text())['platforms']['darwin-aarch64']['url']
assert a != b
assert (root / a).read_bytes() == b'bundle'
assert (root / b).read_bytes() == b'newer'
PY
# Identical bytes are a no-op, preserving even the feed's publication time.
# Give the old feed a distinct date so this assertion never depends on a clock tick.
python3 - "$LOCAL_TEST_DIR/latest.json" <<'PY'
import json, pathlib, sys
p = pathlib.Path(sys.argv[1])
feed = json.loads(p.read_text())
feed['pub_date'] = '2000-01-01T00:00:00Z'
p.write_text(json.dumps(feed))
PY
cp "$LOCAL_TEST_DIR/latest.json" "$ROOT/noop.json"
"$PUBLISH" 9.9.2 --port 5999 --staging-dir "$ROOT/staging" > /dev/null
cmp "$LOCAL_TEST_DIR/latest.json" "$ROOT/noop.json"
# Equal versions may never acquire different bytes.
printf different > "$ROOT/staging/Lucidos.app.tar.gz"
RELEASE_STAGING_PLATFORM_KEY=darwin-aarch64 release_staging_write_manifest \
    "$ROOT/staging" 9.9.2 deadbeef Lucidos.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
if "$PUBLISH" 9.9.2 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
    echo 'FAIL: equal version accepted new bytes' >&2; exit 1
fi
cmp "$LOCAL_TEST_DIR/latest.json" "$ROOT/noop.json"
# Numeric rc ordering, downgrade refusal and final promotion.
for version in 9.10.0-rc.2 9.10.0-rc.10 9.10.0; do
    RELEASE_STAGING_PLATFORM_KEY=darwin-aarch64 release_staging_write_manifest \
        "$ROOT/staging" "$version" deadbeef Lucidos.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
    "$PUBLISH" "$version" --port 5999 --staging-dir "$ROOT/staging" > /dev/null
    if [ "$version" = 9.10.0-rc.10 ]; then
        cp "$LOCAL_TEST_DIR/latest.json" "$ROOT/rc10.json"
        RELEASE_STAGING_PLATFORM_KEY=darwin-aarch64 release_staging_write_manifest \
            "$ROOT/staging" 9.10.0-rc.2 deadbeef Lucidos.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
        if "$PUBLISH" 9.10.0-rc.2 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
            echo 'FAIL: rc.2 replaced rc.10' >&2; exit 1
        fi
        cmp "$LOCAL_TEST_DIR/latest.json" "$ROOT/rc10.json"
    fi
done
cp "$LOCAL_TEST_DIR/latest.json" "$ROOT/final.json"
RELEASE_STAGING_PLATFORM_KEY=darwin-aarch64 release_staging_write_manifest \
    "$ROOT/staging" 9.10.0-rc.11 deadbeef Lucidos.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
if "$PUBLISH" 9.10.0-rc.11 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
    echo 'FAIL: lower version accepted' >&2; exit 1
fi
grep -q '9.10.0-rc.11.*9.10.0' "$ROOT/error"
cmp "$LOCAL_TEST_DIR/latest.json" "$ROOT/final.json"
# A mismatched requested version and an unsupported platform preserve the feed.
cp "$LOCAL_TEST_DIR/latest.json" "$ROOT/before.json"
if "$PUBLISH" 9.9.3 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
    echo 'FAIL: mismatched version accepted' >&2; exit 1
fi
cmp "$LOCAL_TEST_DIR/latest.json" "$ROOT/before.json"
RELEASE_STAGING_PLATFORM_KEY=linux-x86_64 release_staging_write_manifest \
    "$ROOT/staging" 9.9.1 deadbeef Lucidos.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
if "$PUBLISH" 9.9.1 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
    echo 'FAIL: non-macOS staging accepted' >&2; exit 1
fi
cmp "$LOCAL_TEST_DIR/latest.json" "$ROOT/before.json"
RELEASE_STAGING_PLATFORM_KEY=darwin-aarch64 release_staging_write_manifest \
    "$ROOT/staging" 9.9.1 deadbeef Lucidos.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
cp "$LOCAL_TEST_DIR/latest.json" "$ROOT/before.json"
printf tampered > "$ROOT/staging/Lucidos.app.tar.gz"
if "$PUBLISH" 9.9.1 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
    echo 'FAIL: tampered staging accepted' >&2; exit 1
fi
cmp "$LOCAL_TEST_DIR/latest.json" "$ROOT/before.json"
# A changed signature must fail the staging hash check before publication.
RELEASE_STAGING_PLATFORM_KEY=darwin-aarch64 release_staging_write_manifest \
    "$ROOT/staging" 9.9.1 deadbeef Lucidos.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
printf changed-signature > "$ROOT/staging/Lucidos.app.tar.gz.sig"
if "$PUBLISH" 9.9.1 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
    echo 'FAIL: tampered signature accepted' >&2; exit 1
fi
grep -q 'Lucidos.app.tar.gz.sig' "$ROOT/error"
cmp "$LOCAL_TEST_DIR/latest.json" "$ROOT/before.json"
# A DMG-only manifest must not authorize an unlisted updater on disk.
RELEASE_STAGING_PLATFORM_KEY=darwin-aarch64 release_staging_write_manifest \
    "$ROOT/staging" 9.9.1 deadbeef Lucidos.dmg
if "$PUBLISH" 9.9.1 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
    echo 'FAIL: unlisted updater accepted' >&2; exit 1
fi
cmp "$LOCAL_TEST_DIR/latest.json" "$ROOT/before.json"
# An agent uses its own engine's read route, with no explicit gateway port.
cat > "$ROOT/bin/lucidos" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$LOCAL_TEST_CALLS"
[ "$*" = 'update-channel show' ] || exit 1
printf 'Update channel: stable\nLocal staging directory: %s\n' "$LOCAL_TEST_DIR"
SH
export LOCAL_TEST_DIR="$ROOT/agent gateway/local"
RELEASE_STAGING_PLATFORM_KEY=darwin-aarch64 release_staging_write_manifest \
    "$ROOT/staging" 9.10.1 deadbeef Lucidos.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
"$PUBLISH" 9.10.1 --staging-dir "$ROOT/staging" > "$ROOT/out"
[ -f "$LOCAL_TEST_DIR/latest.json" ]
grep -qx 'update-channel show' "$LOCAL_TEST_CALLS"
# No guessed path when the gateway cannot report its staging directory.
cat > "$ROOT/bin/lucidos" <<'SH'
#!/usr/bin/env bash
printf 'Update channel: stable\n'
SH
if "$PUBLISH" 9.9.1 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
    echo 'FAIL: missing gateway directory accepted' >&2; exit 1
fi
printf 'local_channel: passed\n'
