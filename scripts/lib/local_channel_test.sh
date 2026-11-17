#!/usr/bin/env bash
# Offline Local publisher tests: all gateway reads use a fake CLI.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
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
python3 - "$LOCAL_TEST_DIR" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
feed = json.loads((root / 'latest.json').read_text())
assert feed['version'] == '9.9.1'
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
    "$ROOT/staging" 9.9.1 deadbeef Lucidos.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
"$PUBLISH" 9.9.1 --port 5999 --staging-dir "$ROOT/staging" > /dev/null
python3 - "$LOCAL_TEST_DIR" "$ROOT/first.json" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
a = json.loads(pathlib.Path(sys.argv[2]).read_text())['platforms']['darwin-aarch64']['url']
b = json.loads((root / 'latest.json').read_text())['platforms']['darwin-aarch64']['url']
assert a != b
assert (root / a).read_bytes() == b'bundle'
assert (root / b).read_bytes() == b'newer'
PY
# A mismatched requested version and an unsupported platform preserve the feed.
cp "$LOCAL_TEST_DIR/latest.json" "$ROOT/before.json"
if "$PUBLISH" 9.9.2 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
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
# A DMG-only manifest must not authorize an unlisted updater on disk.
RELEASE_STAGING_PLATFORM_KEY=darwin-aarch64 release_staging_write_manifest \
    "$ROOT/staging" 9.9.1 deadbeef Lucidos.dmg
if "$PUBLISH" 9.9.1 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
    echo 'FAIL: unlisted updater accepted' >&2; exit 1
fi
cmp "$LOCAL_TEST_DIR/latest.json" "$ROOT/before.json"
# No guessed path when the gateway cannot report its staging directory.
cat > "$ROOT/bin/lucidos" <<'SH'
#!/usr/bin/env bash
printf 'Update channel: stable\n'
SH
if "$PUBLISH" 9.9.1 --port 5999 --staging-dir "$ROOT/staging" > "$ROOT/error" 2>&1; then
    echo 'FAIL: missing gateway directory accepted' >&2; exit 1
fi
printf 'local_channel: passed\n'
