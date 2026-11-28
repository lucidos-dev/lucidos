#!/usr/bin/env bash
# release_upload.sh: attach a release's assets in an order that never advertises
# a payload before it exists.
#
# ── WHY THIS EXISTS: the v0.16.0 window ──────────────────────────────────────
# `upload_staged_assets` used to attach all four assets in ONE `gh release
# upload` call:
#
#     gh release upload "$tag" --repo "$slug" --clobber \
#         "$dmg" "$app_tarball" "$app_sig" "$latest_json"
#
# `gh` uploads concurrently, so the small files finish first, and `latest.json`
# is by far the smallest. The in-app updater reads
# `…/releases/latest/download/latest.json`, GitHub marks a release "latest" the
# moment it is published (which happens BEFORE this upload runs), and the
# manifest names a `Lucidos.app.tar.gz` on the same tag. So there is a window in
# which the manifest is fully readable and advertises a payload GitHub answers
# with a 404, and every update check landing in it fails.
#
# Measured from the GitHub API's per-asset created_at / updated_at (F8 in
# docs/audits/2026-08-02-macos-update-path-audit.md): 10 s on v0.19.0, 65 s on
# v0.15.0, and on v0.16.0 the whole DMG trio was first uploaded EIGHT HOURS AND
# SIX MINUTES after the release was published, so for that entire window the
# latest release had no `latest.json` asset at all and every packaged client's
# update check failed.
#
# The ordering is therefore made an explicit property of the code rather than an
# accident of which file happens to be smallest: upload the artifacts, PROVE they
# are on the release, then upload the manifest that points at them. It costs one
# extra API call.
#
# ── The residual this does NOT close, and why not ────────────────────────────
# `--clobber` deletes an asset before uploading its replacement. On a CORRECTIVE
# re-upload (`release.sh --attach-notarized` swapping in the stapled DMG) the
# release already carries a `latest.json`, and it stays readable while the
# artifacts beneath it are being replaced. So a client that fetches the manifest
# during those seconds can still meet a 404 on the payload.
#
# That is a real window and it is deliberately left open, because the two obvious
# closes are both worse:
#
#   • Removing the manifest first and restoring it after would turn a POSSIBLE
#     404 on the payload into a GUARANTEED one on the manifest, for the whole
#     upload rather than part of it. Every update check in the window would fail
#     instead of only the ones that get as far as downloading.
#   • Skipping the re-upload of artifacts already on the release would need an
#     identity test, and GitHub exposes no checksum for an asset. Name plus size
#     is a proxy, and the failure it admits (shipping a stale updater payload
#     because the bytes changed at an identical size) is far worse than a
#     transient 404.
#
# The proportion matters: the window this leaves is one artifact upload long
# (~10 s for a 70 MB payload) on a rare corrective action, against the 8h06m of
# total unavailability on a first publish that the ordering below removes. Closing
# it properly needs the re-attach to upload only the DMG, which is a change to
# what release.sh asks for rather than to how this function honours it.
#
# ── Public-mirror safety ─────────────────────────────────────────────────────
# Sourced unconditionally by build-dmg.sh, so it must stay OUT of
# RELEASE_TREE_EXCLUDE_PATHS, exactly like updater_payload.sh and release_dmg.sh.
#
# The verdict is a pure function over `gh release view --json assets` output, so
# the whole decision is unit-testable offline against captured shapes, and the
# effectful half is drivable with a fake `gh` on PATH (the same technique
# service_test.sh uses for launchctl). Unit tests:
# scripts/lib/release_upload_test.sh.

# How hard to try before believing an asset really is missing. `gh release
# upload` returns after the upload completes, so one query normally suffices, but
# the audit's created_at/updated_at skew says GitHub's asset state is not
# instantaneous. A short bounded re-check absorbs a blip without turning a real
# failure into a long hang.
RELEASE_UPLOAD_VERIFY_ATTEMPTS="${RELEASE_UPLOAD_VERIFY_ATTEMPTS:-5}"
RELEASE_UPLOAD_VERIFY_INTERVAL="${RELEASE_UPLOAD_VERIFY_INTERVAL:-3}"

# release_upload_assets_present <name> <size> [<name> <size>…]: JSON on stdin,
# zero when every named asset is on the release, fully uploaded and the right
# size. Prints one line per unsatisfied expectation to stderr.
#
# Three ways an expectation fails, told apart because they need different human
# responses:
#   • absent   the upload did not land at all;
#   • wrong size   the upload landed truncated, which is the failure a bare
#     "is the name there" check cannot see;
#   • state present and not `uploaded`   GitHub is still processing it.
#
# A MISSING `state` field is accepted rather than refused: it is advisory, and a
# future `gh` that stopped emitting it must not break every release. The size
# comparison is the load-bearing check and does not depend on it.
#
# Fail-closed on the input itself: unparseable JSON, a non-object, or a missing
# `assets` array is a refusal, never a vacuous pass.
release_upload_assets_present() {
    [ "$#" -gt 0 ] && [ $(( $# % 2 )) -eq 0 ] || {
        echo "ERROR: release_upload_assets_present needs <name> <size> pairs" >&2
        return 1
    }
    # The pairs go through argv, not a space-joined variable: an asset name with
    # a space in it would re-pair every name with the wrong size and report
    # nonsense. No shipped artifact has one today, which is exactly why the trap
    # would sit unnoticed.
    python3 -c '
import json, sys

try:
    data = json.load(sys.stdin)
except ValueError as exc:
    sys.stderr.write("ERROR: could not parse the release asset listing: %s\n" % exc)
    sys.exit(1)
if not isinstance(data, dict) or not isinstance(data.get("assets"), list):
    sys.stderr.write("ERROR: the release asset listing has no assets array\n")
    sys.exit(1)

found = {}
for asset in data["assets"]:
    if isinstance(asset, dict) and isinstance(asset.get("name"), str):
        found[asset["name"]] = asset

words = sys.argv[1:]
missing = 0
for name, size in zip(words[0::2], words[1::2]):
    asset = found.get(name)
    if asset is None:
        sys.stderr.write("ERROR: %s is not on the release yet\n" % name)
        missing += 1
        continue
    state = asset.get("state")
    if state is not None and state != "uploaded":
        sys.stderr.write("ERROR: %s is on the release but its state is %r, not \"uploaded\"\n" % (name, state))
        missing += 1
        continue
    actual = asset.get("size")
    if actual != int(size):
        sys.stderr.write("ERROR: %s is on the release at %s bytes, expected %s\n" % (name, actual, size))
        missing += 1
sys.exit(1 if missing else 0)
' "$@"
}

# release_upload_file_size <path>: the byte count, via `wc -c` rather than
# `stat`, whose flags differ between BSD and GNU.
release_upload_file_size() {
    wc -c < "$1" | tr -d '[:space:]'
}

# release_upload_verify_present <tag> <repo> <path>…: re-read the release from
# GitHub and assert every <path> is there at its local size. Bounded retry (see
# the knobs above); the final attempt's reasons are what reach the operator.
release_upload_verify_present() {
    local tag="$1" repo="$2"
    shift 2
    local path attempt=1 listing
    local -a expected=()
    for path in "$@"; do
        expected+=("$(basename "$path")" "$(release_upload_file_size "$path")")
    done

    while :; do
        listing="$(gh release view "$tag" --repo "$repo" --json assets 2>/dev/null || true)"
        if [ -n "$listing" ] \
           && printf '%s' "$listing" | release_upload_assets_present "${expected[@]}" 2>/dev/null; then
            return 0
        fi
        if [ "$attempt" -ge "$RELEASE_UPLOAD_VERIFY_ATTEMPTS" ]; then
            # Out of attempts. Re-run the predicate with its stderr showing, so
            # the operator learns WHICH asset is missing or short rather than
            # only that something was. Its exit code is discarded because the
            # verdict is already decided.
            if [ -z "$listing" ]; then
                echo "ERROR: could not read the asset list of $tag from $repo." >&2
            else
                printf '%s' "$listing" | release_upload_assets_present "${expected[@]}" || true
            fi
            return 1
        fi
        attempt=$((attempt + 1))
        sleep "$RELEASE_UPLOAD_VERIFY_INTERVAL"
    done
}

# release_upload_write_latest_json <out> <version> <platform-key> <url> <pub-date>
#     <sig-file> [<notes-file>]: write the in-app updater manifest.
#
# The PLATFORM KEY IS A PARAMETER AND IS NEVER DERIVED HERE (F10 in
# docs/audits/2026-08-02-macos-update-path-audit.md). It used to come from a
# `case "$(uname -m)"` at the call site, which describes the machine running the
# upload rather than the artifact being uploaded, and the mislabelling is silent:
# an updater whose target key is absent from `platforms` reports "no update"
# rather than an error, so a `--release-attach` from the wrong host would strand
# every client with no signal at all. The honest key is derived from the staged
# Mach-O at BUILD time and recorded in the staging manifest
# (`release_staging_platform_key_for_binary`, `release_staging_platform_key`), so
# this function's job is to refuse an empty one and otherwise use what it is
# given. There is no `uname` on this path, and the tests assert that.
#
# The asset's name is its basename and the updater endpoint resolves
# `…/releases/latest/download/latest.json`, so <out> must literally be named
# latest.json. python3 does the JSON encoding: the notes are a multi-line
# changelog section and the signature is base64, and neither survives hand-rolled
# quoting. The notes file is optional (empty notes when it is absent or unset).
release_upload_write_latest_json() {
    local out="$1" version="$2" platform_key="$3" url="$4" pub_date="$5" sig_file="$6" notes_file="${7:-}"

    [ -n "$out" ]          || { echo "ERROR: release_upload_write_latest_json needs an output path" >&2; return 1; }
    [ -n "$version" ]      || { echo "ERROR: release_upload_write_latest_json needs a version" >&2; return 1; }
    [ -n "$platform_key" ] || {
        echo "ERROR: refusing to write $(basename "$out") with no platform key." >&2
        echo "       The key says which platforms entry the updater looks itself up under, and" >&2
        echo "       an absent entry makes every client report 'no update' rather than fail." >&2
        echo "       It comes from the staging manifest; see release_staging_platform_key." >&2
        return 1
    }
    [ -n "$url" ]          || { echo "ERROR: release_upload_write_latest_json needs a download URL" >&2; return 1; }
    [ -n "$pub_date" ]     || { echo "ERROR: release_upload_write_latest_json needs a pub_date" >&2; return 1; }
    [ -f "$sig_file" ]     || { echo "ERROR: no updater signature file to read: $sig_file" >&2; return 1; }
    if [ -n "$notes_file" ] && [ ! -f "$notes_file" ]; then
        echo "ERROR: no such notes file: $notes_file" >&2
        return 1
    fi

    RELEASE_VERSION="$version" PLATFORM_KEY="$platform_key" DOWNLOAD_URL="$url" \
    PUB_DATE="$pub_date" NOTES_FILE="$notes_file" SIG_FILE="$sig_file" \
    python3 - > "$out" <<'PY'
import json, os
notes_file = os.environ.get("NOTES_FILE", "")
notes = open(notes_file, encoding="utf-8").read().strip() if notes_file else ""
sig = open(os.environ["SIG_FILE"], encoding="utf-8").read().strip()
manifest = {
    "version": os.environ["RELEASE_VERSION"],
    "notes": notes,
    "pub_date": os.environ["PUB_DATE"],
    "platforms": {
        os.environ["PLATFORM_KEY"]: {
            "signature": sig,
            "url": os.environ["DOWNLOAD_URL"],
        }
    },
}
print(json.dumps(manifest, indent=2))
PY
    [ -s "$out" ] || { echo "ERROR: latest.json generation produced no output" >&2; return 1; }
}

# release_upload_artifacts_then_manifest <tag> <repo> <manifest> <artifact>…:
# THE ORDERING. Upload the artifacts, prove they are present on the release, and
# only then upload the manifest that points at them.
#
# The manifest is a separate parameter rather than just the last artifact so the
# ordering is a property of the SIGNATURE: there is no argument list that puts
# `latest.json` in the first batch.
release_upload_artifacts_then_manifest() {
    local tag="$1" repo="$2" manifest="$3"
    shift 3
    local path

    [ -n "$tag" ]  || { echo "ERROR: release_upload_artifacts_then_manifest needs a release tag" >&2; return 1; }
    [ -n "$repo" ] || { echo "ERROR: release_upload_artifacts_then_manifest needs a repo slug" >&2; return 1; }
    [ "$#" -gt 0 ] || { echo "ERROR: release_upload_artifacts_then_manifest needs at least one artifact" >&2; return 1; }
    for path in "$@" "$manifest"; do
        [ -f "$path" ] || { echo "ERROR: no such file to upload: $path" >&2; return 1; }
    done

    gh release upload "$tag" --repo "$repo" --clobber "$@" \
        || { echo "ERROR: gh release upload failed for $tag (artifacts)" >&2; return 1; }

    release_upload_verify_present "$tag" "$repo" "$@" || {
        echo "ERROR: refusing to upload $(basename "$manifest") for $tag: the assets it points at are not all on the release." >&2
        echo "       That is the v0.16.0 failure, where the manifest advertised a payload GitHub answered with a 404 for eight hours." >&2
        echo "       Re-run the upload once the artifacts above have landed." >&2
        return 1
    }

    gh release upload "$tag" --repo "$repo" --clobber "$manifest" \
        || { echo "ERROR: gh release upload failed for $tag ($(basename "$manifest"))" >&2; return 1; }
}
