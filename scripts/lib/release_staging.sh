#!/usr/bin/env bash
# release_staging.sh — stage release artifacts + a verifiable manifest for the
# build-once / verify-then-publish flow.
#
# The build-once model splits a release into a private BUILD phase and a public
# PUBLISH phase so the DMG you manually verify is bit-for-bit the DMG that ships:
#   • build-dmg.sh --release-build  stages the produced artifacts (the .dmg, the
#     updater .app.tar.gz, and its .sig) into .lucidos/release-staging/<version>/
#     and writes manifest.json — NO upload, NO GitHub Release required.
#   • build-dmg.sh --release-attach (and release.sh --publish-verified) VERIFY
#     that staging before doing anything public, then upload the STAGED artifacts
#     with no rebuild.
#
# manifest.json shape:
#   { "version": "<N.N.N>",
#     "source_commit": "<git rev-parse HEAD of the tree that was built>",
#     "artifacts": [ { "name": "<basename>", "sha256": "<hex>" }, … ] }
#
# Pure shell + python3 (hashlib for sha256, json for the manifest) — no git, gh,
# or network — so the unit tests (scripts/lib/release_staging_test.sh) exercise it
# fully offline. The "artifacts" are the staged FILES; this lib never builds or
# signs anything. It carries nothing machine-specific, so it is NOT stripped from
# the public mirror (build-dmg.sh's --release-build is a legitimate public path);
# contrast release_signing.sh / release_events.sh, which ARE in EXCLUDE_PATHS.

# release_staging_sha256 <file> — print the lowercase hex SHA-256 of <file>.
release_staging_sha256() {
    python3 - "$1" <<'PY'
import hashlib, sys
h = hashlib.sha256()
with open(sys.argv[1], "rb") as f:
    for chunk in iter(lambda: f.read(1 << 20), b""):
        h.update(chunk)
print(h.hexdigest())
PY
}

# release_staging_write_manifest <dir> <version> <source_commit> <name…>
# Write <dir>/manifest.json describing the already-staged artifacts <name…> (each
# must exist at <dir>/<name>). sha256 is computed over the staged bytes.
release_staging_write_manifest() {
    local dir="$1" version="$2" commit="$3"
    shift 3
    [ -d "$dir" ] || { echo "ERROR: staging dir '$dir' does not exist" >&2; return 1; }
    [ "$#" -ge 1 ] || { echo "ERROR: release_staging_write_manifest needs at least one artifact name" >&2; return 1; }
    local name
    for name in "$@"; do
        [ -f "$dir/$name" ] || { echo "ERROR: staged artifact '$dir/$name' is missing" >&2; return 1; }
    done
    DIR="$dir" VERSION="$version" COMMIT="$commit" python3 - "$@" <<'PY'
import hashlib, json, os, sys

def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()

dir_ = os.environ["DIR"]
artifacts = [
    {"name": name, "sha256": sha256(os.path.join(dir_, name))}
    for name in sys.argv[1:]
]
manifest = {
    "version": os.environ["VERSION"],
    "source_commit": os.environ["COMMIT"],
    "artifacts": artifacts,
}
with open(os.path.join(dir_, "manifest.json"), "w", encoding="utf-8") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
PY
}

# release_staging_manifest_field <dir> <version|source_commit> — print the field.
# Non-zero if the manifest is missing or the field is absent.
release_staging_manifest_field() {
    local dir="$1" field="$2"
    [ -f "$dir/manifest.json" ] || { echo "ERROR: no manifest.json in '$dir'" >&2; return 1; }
    FIELD="$field" python3 - "$dir/manifest.json" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as f:
    manifest = json.load(f)
field = os.environ["FIELD"]
if field not in manifest:
    sys.stderr.write("ERROR: manifest.json has no '%s' field\n" % field)
    sys.exit(1)
print(manifest[field])
PY
}

# release_staging_verify <dir> — confirm the manifest is present, every listed
# artifact exists, and each artifact's recomputed sha256 matches the manifest.
# Prints a clear error to stderr and returns non-zero on the first problem. This
# is the integrity half of the guard (build-dmg.sh --release-attach runs it before
# any upload; release.sh --publish-verified runs it before going public).
release_staging_verify() {
    local dir="$1"
    [ -f "$dir/manifest.json" ] || { echo "ERROR: staging manifest not found: $dir/manifest.json" >&2; return 1; }
    DIR="$dir" python3 - "$dir/manifest.json" <<'PY'
import hashlib, json, os, sys

def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()

dir_ = os.environ["DIR"]
with open(sys.argv[1], encoding="utf-8") as f:
    manifest = json.load(f)

artifacts = manifest.get("artifacts")
if not isinstance(artifacts, list) or not artifacts:
    sys.stderr.write("ERROR: staging manifest lists no artifacts\n")
    sys.exit(1)

for art in artifacts:
    name = art.get("name") if isinstance(art, dict) else None
    want = art.get("sha256") if isinstance(art, dict) else None
    if not name or not want:
        sys.stderr.write("ERROR: malformed manifest entry: %r\n" % (art,))
        sys.exit(1)
    path = os.path.join(dir_, name)
    if not os.path.isfile(path):
        sys.stderr.write("ERROR: staged artifact missing: %s\n" % path)
        sys.exit(1)
    got = sha256(path)
    if got != want:
        sys.stderr.write(
            "ERROR: checksum mismatch for %s\n"
            "       manifest: %s\n"
            "       actual:   %s\n" % (name, want, got))
        sys.exit(1)
PY
}

# release_staging_assert_commit <dir> <expected_commit> — the identity guard.
# Non-zero (with a clear message) unless the manifest's source_commit equals
# <expected_commit>. release.sh --publish-verified uses it to refuse publishing a
# staging dir whose verified tree no longer matches the worktree HEAD.
release_staging_assert_commit() {
    local dir="$1" expected="$2" actual
    actual="$(release_staging_manifest_field "$dir" source_commit)" || return 1
    if [ "$actual" != "$expected" ]; then
        echo "ERROR: staging identity mismatch — manifest.source_commit ($actual)" >&2
        echo "       does not match the expected commit ($expected)." >&2
        echo "       The verified build does not correspond to this tree; re-run" >&2
        echo "       --verify-build before publishing." >&2
        return 1
    fi
    return 0
}
