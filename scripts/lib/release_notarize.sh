#!/usr/bin/env bash
# release_notarize.sh — the resume handle for the notarization stage.
#
# Apple's notary service routinely takes longer than the process waiting on it
# lives. build-dmg.sh used to run `notarytool submit … --wait` inline, so losing
# the waiter (watchdog, laptop sleep, Ctrl-C, OOM) threw away a complete build:
# the signed DMG sat on disk with no staple, no staging dir, no manifest.json, and
# the submission UUID existed only in the dead process's stdout. The only recovery
# was a full rebuild — a cargo release build plus ~134 inside-out codesigns.
#
# The fix is to SPLIT submit from wait and persist a resume handle the instant the
# submission id is known, BEFORE any waiting happens. This library owns that
# handle: writing it, reading it back, and deciding whether it may be resumed.
# Everything here is pure (python3 json/hashlib + the filesystem) — no xcrun, no
# network — so scripts/lib/release_notarize_test.sh exercises the whole matrix
# offline.
#
# State file (one per version, under the tree that built the DMG):
#   <repo-root>/.lucidos/release-state/notarize-<version>.json
#   { "submission_id": "<notary UUID>",
#     "dmg_path":      "<absolute path to the signed DMG that was submitted>",
#     "version":       "<N.N.N>",
#     "dmg_sha256":    "<hex sha256 of the DMG at submit time>",
#     "source_commit": "<git HEAD of the tree that was built>",
#     "submitted_at":  "<UTC ISO-8601; adoption time for --adopt-submission>" }
#
# The resume gate (release_notarize_resumable) is deliberately strict: the DMG on
# disk must still hash to what was submitted, and the recorded source_commit must
# still be the tree's HEAD. The second check is not paranoia — the resumed run is
# what writes the staging manifest.json, and it stamps source_commit from ITS OWN
# HEAD. Resuming on a moved tree would therefore publish a manifest claiming a
# commit the DMG was never built from, and release.sh --publish-verified's identity
# guard would then pass on a lie.
#
# Depends on release_staging_sha256, so source this AFTER release_staging.sh (the
# same ordering headless_tarball.sh already relies on). Like release_staging.sh it
# carries nothing machine-specific and build-dmg.sh (which ships) needs it, so it
# is NOT stripped from the public mirror — contrast release_signing.sh /
# release_events.sh, which ARE in EXCLUDE_PATHS.

# release_notarize_state_path <repo-root> <version> — the state file for <version>.
release_notarize_state_path() {
    printf '%s/.lucidos/release-state/notarize-%s.json' "$1" "$2"
}

# release_notarize_json_field <field> — print <field> from a JSON object on stdin.
# Empty output when the field is absent or null; non-zero only when stdin is not a
# JSON object. Used to read notarytool's `--output-format json` replies.
release_notarize_json_field() {
    FIELD="$1" python3 -c '
import json, os, sys
try:
    data = json.load(sys.stdin)
except ValueError:
    sys.stderr.write("ERROR: expected JSON on stdin\n")
    sys.exit(1)
if not isinstance(data, dict):
    sys.stderr.write("ERROR: expected a JSON object on stdin\n")
    sys.exit(1)
value = data.get(os.environ["FIELD"])
print("" if value is None else value)
'
}

# release_notarize_write_state <path> <submission-id> <dmg> <version> <sha256>
#                              <source-commit> <submitted-at>
# Write the resume handle. The write is atomic (temp file + fsync + os.replace):
# this file exists precisely so a process can be killed at any moment, so it must
# never be observable half-written.
release_notarize_write_state() {
    local path="$1" id="$2" dmg="$3" version="$4" sha="$5" commit="$6" ts="$7"
    [ -n "$path" ] || { echo "ERROR: release_notarize_write_state needs a state-file path" >&2; return 1; }
    [ -n "$id" ]   || { echo "ERROR: release_notarize_write_state needs a submission id" >&2; return 1; }
    mkdir -p "$(dirname "$path")" \
        || { echo "ERROR: cannot create the release-state dir for $path" >&2; return 1; }
    ID="$id" DMG="$dmg" VERSION="$version" SHA="$sha" COMMIT="$commit" TS="$ts" \
    python3 - "$path" <<'PY'
import json, os, sys, tempfile

path = sys.argv[1]
state = {
    "submission_id": os.environ["ID"],
    "dmg_path": os.environ["DMG"],
    "version": os.environ["VERSION"],
    "dmg_sha256": os.environ["SHA"],
    "source_commit": os.environ["COMMIT"],
    "submitted_at": os.environ["TS"],
}
directory = os.path.dirname(path) or "."
fd, tmp = tempfile.mkstemp(dir=directory, prefix=".notarize-state-")
try:
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        json.dump(state, f, indent=2)
        f.write("\n")
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)
except BaseException:
    try:
        os.unlink(tmp)
    except OSError:
        pass
    raise
PY
}

# release_notarize_field <path> <field> — print one field of the state file.
# Non-zero (with a message) when the file is missing, unreadable, or lacks it.
release_notarize_field() {
    local path="$1" field="$2"
    [ -f "$path" ] || { echo "ERROR: no notarize state at '$path'" >&2; return 1; }
    FIELD="$field" python3 - "$path" <<'PY'
import json, os, sys
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        state = json.load(f)
except (OSError, ValueError) as exc:
    sys.stderr.write("ERROR: unreadable notarize state %s: %s\n" % (sys.argv[1], exc))
    sys.exit(1)
field = os.environ["FIELD"]
if not isinstance(state, dict) or field not in state:
    sys.stderr.write("ERROR: notarize state has no '%s' field\n" % field)
    sys.exit(1)
print(state[field])
PY
}

# release_notarize_resumable <path> <expected-commit> — the resume gate.
# Zero when the handle may be picked up: the state parses, carries every required
# field, its DMG still exists, that DMG still hashes to what was submitted, and
# <expected-commit> is the recorded source_commit. Otherwise prints exactly why to
# stderr and returns non-zero — callers surface that reason either as a hard
# failure (an explicit --resume-notarize) or as a note before rebuilding (the
# automatic detection in a build-grade run).
release_notarize_resumable() {
    local path="$1" expected="$2" dmg recorded_sha actual_sha recorded_commit id

    [ -n "$path" ] || { echo "ERROR: release_notarize_resumable needs a state-file path" >&2; return 1; }
    [ -f "$path" ] || { echo "ERROR: no notarize state at $path" >&2; return 1; }
    [ -n "$expected" ] \
        || { echo "ERROR: release_notarize_resumable needs the expected source commit" >&2; return 1; }

    id="$(release_notarize_field "$path" submission_id)" || return 1
    [ -n "$id" ] || { echo "ERROR: notarize state $path records no submission id" >&2; return 1; }

    dmg="$(release_notarize_field "$path" dmg_path)" || return 1
    if [ -z "$dmg" ] || [ ! -f "$dmg" ]; then
        echo "ERROR: the submitted DMG is gone: '$dmg'" >&2
        echo "       (recorded by $path for submission $id)" >&2
        return 1
    fi

    recorded_sha="$(release_notarize_field "$path" dmg_sha256)" || return 1
    actual_sha="$(release_staging_sha256 "$dmg")" \
        || { echo "ERROR: could not hash $dmg" >&2; return 1; }
    if [ "$actual_sha" != "$recorded_sha" ]; then
        echo "ERROR: checksum mismatch — $dmg is not the DMG that was submitted." >&2
        echo "       submitted: $recorded_sha" >&2
        echo "       on disk:   $actual_sha" >&2
        echo "       Apple scanned different bytes; refusing to staple/stage these." >&2
        return 1
    fi

    recorded_commit="$(release_notarize_field "$path" source_commit)" || return 1
    if [ "$recorded_commit" != "$expected" ]; then
        echo "ERROR: source-commit mismatch — the tree moved since the DMG was built." >&2
        echo "       built from: $recorded_commit" >&2
        echo "       tree is at: $expected" >&2
        echo "       Resuming would stamp a staging manifest with a commit the DMG" >&2
        echo "       was not built from; re-build instead." >&2
        return 1
    fi
    return 0
}

# release_notarize_clear <path> — drop a spent resume handle. A no-op when the
# path is empty or the file is already gone, so callers need no guard.
release_notarize_clear() {
    [ -n "${1:-}" ] || return 0
    rm -f "$1"
}

# release_notarize_valid_submission_id <string> — notary submission ids are UUIDs.
# Shape-checked so a fat-fingered --adopt-submission fails at argument parsing
# rather than after a credential round-trip to Apple.
release_notarize_valid_submission_id() {
    case "${1:-}" in
        [0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]-[0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]-[0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]-[0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]-[0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F])
            return 0 ;;
        *)  return 1 ;;
    esac
}
