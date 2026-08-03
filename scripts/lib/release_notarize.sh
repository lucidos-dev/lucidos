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
# ── TWO STAGES, ONE HANDLE ───────────────────────────────────────────────────
# A release makes TWO notary submissions, in Apple's documented order:
#
#   stage `app`   a zip of the signed .app. Its ticket is what lets the app be
#                 STAPLED before the DMG is built around it, so the copy a user
#                 drags to /Applications carries a ticket and its first launch
#                 does not need Apple reachable. (F5: no shipped DMG's app had
#                 one, because the only copy that ever got stapled was the
#                 standalone build output, which is never shipped.)
#   stage `dmg`   the finished disk image, the artifact a browser downloads and
#                 the only one Gatekeeper assesses on that path.
#
# They cannot overlap: the DMG has to be built FROM the stapled app. One handle
# carries both, with `stage` naming which submission is outstanding, because an
# operator resumes "the notarization of this release" without caring which half died.
# The resume branches on `stage`, and the `app` branch runs on into the DMG half.
#
# `artifact_path` / `artifact_sha256` therefore name whichever file was handed to
# notarytool: the app zip at stage `app`, the DMG at stage `dmg`. They were
# `dmg_path` / `dmg_sha256` until 2026-08-02, when the app stage made that name
# a lie at one of the two stages.
#
# `artifact_sha256` is the bytes the submission is ACCOUNTABLE for, which are the
# submitted bytes right up until the run staples its own ticket into them.
# `xcrun stapler staple` writes the ticket INTO the DMG, so build-dmg.sh's
# notarize_carry_staple_into_handle moves this field forward to the post-staple
# bytes at that moment, in step with the in-memory expectation and the pin. It
# has to: the gate below re-hashes the artifact against this field and refuses on
# any mismatch, so a handle left describing the pre-staple bytes would make a
# just-stapled release unresumable, and on a deferred release that means an
# already-published DMG no attach could ever staple (v0.19.1). Only the `dmg`
# stage staples through that path, so the `app` stage's record of the submitted
# zip is never rewritten.
#
# ── THE PAIRED SET (F3) ──────────────────────────────────────────────────────
# The handle also records the updater trio: `updater_tarball_path`, its sha256,
# and the sha256 of its `.sig` (always `<tarball>.sig`, so the path is derived
# rather than stored twice and unable to disagree).
#
# WHY: staging pairs the recorded artifact with the `.app.tar.gz` + `.sig` it
# finds under the bundle dir, and nothing tied those to the build that produced
# the artifact. Worse, the recovery branch of assert_submitted_artifacts_are_intact
# RESTORES the DMG from its pin when a concurrent rebuild overwrote it, and that
# is exactly the state in which the tarball on disk belongs to the NEWER build.
# The staging manifest then records both, release_staging_verify finds them
# self-consistent (it only checks internal consistency), and the release ships a
# DMG and an updater payload from two different builds. The 2026-07-28
# three-concurrent-pollers incident is the precondition, so the tree has been in
# that state.
#
# The pairing is recorded at the FIRST submit and carried forward unchanged. The
# app stage submits first, so recording it only at the DMG submit would let a
# tarball clobbered during the app's wait be adopted as "the pairing": once again
# self-consistent, and wrong.
#
# A handle written before 2026-08-02 has none of these keys. That is refused by
# name rather than read as "nothing to compare, so it matches". See
# release_notarize_resumable.
#
# State file (one per version, under the tree that built it):
#   <repo-root>/.lucidos/release-state/notarize-<version>.json
#   { "stage":                  "app" | "dmg",
#     "submission_id":          "<notary UUID>",
#     "artifact_path":          "<absolute path to the file that was submitted>",
#     "artifact_sha256":        "<hex sha256 of that file at submit time>",
#     "version":                "<N.N.N>",
#     "source_commit":          "<git HEAD of the tree that was built>",
#     "submitted_at":           "<UTC ISO-8601; adoption time for --adopt-*>",
#     "app_path":               "<absolute path to the .app; stage app only>",
#     "app_cdhash":             "<the app's CDHash; stage app only>",
#     "updater_tarball_path":   "<absolute path to Lucidos.app.tar.gz>",
#     "updater_tarball_sha256": "<hex sha256>",
#     "updater_sig_sha256":     "<hex sha256 of <tarball>.sig>" }
#
# Every key is ALWAYS written, empty when it does not apply, so "key missing"
# means exactly one thing: a handle from before this shape existed.
#
# The resume gate (release_notarize_resumable) is deliberately strict: the
# submitted artifact must still hash to what was submitted, the updater trio must
# still hash to what was recorded, and the recorded source_commit must still be
# the tree's HEAD. That last check is not paranoia: the resumed run is what
# writes the staging manifest.json, and it stamps source_commit from ITS OWN
# HEAD. Resuming on a moved tree would therefore publish a manifest claiming a
# commit the artifact was never built from, and release.sh --publish-verified's
# identity guard would then pass on a lie.
#
# Depends on release_staging_sha256, so source this AFTER release_staging.sh (the
# same ordering headless_tarball.sh already relies on). Like release_staging.sh it
# carries nothing machine-specific and build-dmg.sh (which ships) needs it, so it
# is NOT stripped from the public mirror — contrast release_signing.sh /
# release_events.sh, which ARE in EXCLUDE_PATHS.

# The two notarization stages, as the handle spells them.
RELEASE_NOTARIZE_STAGE_APP="app"
RELEASE_NOTARIZE_STAGE_DMG="dmg"

# The keys a handle written before the two-stage paired shape (2026-08-02) does
# not carry. Their ABSENCE is what identifies such a handle.
RELEASE_NOTARIZE_REQUIRED_FIELDS="stage artifact_path artifact_sha256 updater_tarball_path updater_tarball_sha256 updater_sig_sha256"

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

# release_notarize_write_state <path> <stage> <submission-id> <artifact>
#                              <artifact-sha256> <version> <source-commit>
#                              <submitted-at>
#
# The five remaining fields arrive through the environment rather than as five
# more positional arguments, the same way release_staging_write_manifest takes
# RELEASE_STAGING_NOTARIZED. They are optional per stage, and a thirteen-argument
# positional call is a miswiring waiting to happen:
#
#   RELEASE_NOTARIZE_APP_PATH               the .app the zip was made from (stage app)
#   RELEASE_NOTARIZE_APP_CDHASH             its code-directory hash        (stage app)
#   RELEASE_NOTARIZE_UPDATER_TARBALL        absolute path to Lucidos.app.tar.gz
#   RELEASE_NOTARIZE_UPDATER_TARBALL_SHA256 its sha256 at submit time
#   RELEASE_NOTARIZE_UPDATER_SIG_SHA256     the sha256 of <tarball>.sig
#
# Unset means empty, and empty is written explicitly, so a reader can always tell
# "this build had no updater payload" from "this handle predates the field".
#
# The write is atomic (temp file + fsync + os.replace): this file exists precisely
# so a process can be killed at any moment, so it must never be observable
# half-written.
release_notarize_write_state() {
    local path="$1" stage="$2" id="$3" artifact="$4" sha="$5" version="$6" commit="$7" ts="$8"
    [ -n "$path" ] || { echo "ERROR: release_notarize_write_state needs a state-file path" >&2; return 1; }
    [ -n "$id" ]   || { echo "ERROR: release_notarize_write_state needs a submission id" >&2; return 1; }
    case "$stage" in
        "$RELEASE_NOTARIZE_STAGE_APP"|"$RELEASE_NOTARIZE_STAGE_DMG") ;;
        *) echo "ERROR: release_notarize_write_state needs stage '$RELEASE_NOTARIZE_STAGE_APP' or '$RELEASE_NOTARIZE_STAGE_DMG', got '$stage'" >&2; return 1 ;;
    esac
    mkdir -p "$(dirname "$path")" \
        || { echo "ERROR: cannot create the release-state dir for $path" >&2; return 1; }
    STAGE="$stage" ID="$id" ARTIFACT="$artifact" SHA="$sha" VERSION="$version" \
    COMMIT="$commit" TS="$ts" \
    APP_PATH="${RELEASE_NOTARIZE_APP_PATH:-}" \
    APP_CDHASH="${RELEASE_NOTARIZE_APP_CDHASH:-}" \
    UPD_TARBALL="${RELEASE_NOTARIZE_UPDATER_TARBALL:-}" \
    UPD_TARBALL_SHA="${RELEASE_NOTARIZE_UPDATER_TARBALL_SHA256:-}" \
    UPD_SIG_SHA="${RELEASE_NOTARIZE_UPDATER_SIG_SHA256:-}" \
    python3 - "$path" <<'PY'
import json, os, sys, tempfile

path = sys.argv[1]
state = {
    "stage": os.environ["STAGE"],
    "submission_id": os.environ["ID"],
    "artifact_path": os.environ["ARTIFACT"],
    "artifact_sha256": os.environ["SHA"],
    "version": os.environ["VERSION"],
    "source_commit": os.environ["COMMIT"],
    "submitted_at": os.environ["TS"],
    "app_path": os.environ["APP_PATH"],
    "app_cdhash": os.environ["APP_CDHASH"],
    "updater_tarball_path": os.environ["UPD_TARBALL"],
    "updater_tarball_sha256": os.environ["UPD_TARBALL_SHA"],
    "updater_sig_sha256": os.environ["UPD_SIG_SHA"],
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

# release_notarize_has_fields <path> <field>…: zero when EVERY named key is
# present in the state file, regardless of whether its value is empty.
#
# Presence and emptiness are different questions and the difference is
# load-bearing: an empty `updater_tarball_path` means "this build produced no
# updater payload", while an ABSENT one means "this handle was written before the
# paired-set shape existed", and only the second may never be treated as a match.
release_notarize_has_fields() {
    local path="$1"
    shift
    [ -f "$path" ] || return 1
    FIELDS="$*" python3 - "$path" <<'PY'
import json, os, sys
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        state = json.load(f)
except (OSError, ValueError):
    sys.exit(1)
if not isinstance(state, dict):
    sys.exit(1)
sys.exit(0 if all(f in state for f in os.environ["FIELDS"].split()) else 1)
PY
}

# release_notarize_updater_sig_path <tarball>: the detached updater signature
# that belongs to <tarball>. Derived rather than stored, because
# updater_payload_resign writes exactly `<tarball>.sig` and Tauri emits it there,
# so a second recorded path could only ever disagree with this one.
release_notarize_updater_sig_path() {
    printf '%s' "${1}.sig"
}

# release_notarize_updater_intact <path>: zero when the updater tarball and
# `.sig` recorded in <path> are still byte-identical to what was recorded.
#
# Vacuously zero when no tarball was recorded (an empty value: a build with no
# updater payload). A handle MISSING the keys entirely is not this function's
# call to make; release_notarize_resumable refuses that before asking.
release_notarize_updater_intact() {
    local path="$1" tarball recorded actual sig sig_recorded
    tarball="$(release_notarize_field "$path" updater_tarball_path)" || return 1
    [ -n "$tarball" ] || return 0

    recorded="$(release_notarize_field "$path" updater_tarball_sha256)" || return 1
    if [ ! -f "$tarball" ]; then
        echo "ERROR: the updater payload recorded with this submission is gone: '$tarball'" >&2
        return 1
    fi
    actual="$(release_staging_sha256 "$tarball")" \
        || { echo "ERROR: could not hash $tarball" >&2; return 1; }
    if [ "$actual" != "$recorded" ]; then
        echo "ERROR: updater-payload mismatch: $(basename "$tarball") is not the tarball this submission was paired with." >&2
        echo "       submitted: $recorded" >&2
        echo "       on disk:   $actual" >&2
        echo "       A later build replaced it. Staging this would ship a DMG and an" >&2
        echo "       updater payload from two different builds." >&2
        return 1
    fi

    sig_recorded="$(release_notarize_field "$path" updater_sig_sha256)" || return 1
    [ -n "$sig_recorded" ] || return 0
    sig="$(release_notarize_updater_sig_path "$tarball")"
    if [ ! -f "$sig" ]; then
        echo "ERROR: the updater signature recorded with this submission is gone: '$sig'" >&2
        return 1
    fi
    actual="$(release_staging_sha256 "$sig")" \
        || { echo "ERROR: could not hash $sig" >&2; return 1; }
    if [ "$actual" != "$sig_recorded" ]; then
        echo "ERROR: updater-signature mismatch: $(basename "$sig") is not the signature this submission was paired with." >&2
        echo "       submitted: $sig_recorded" >&2
        echo "       on disk:   $actual" >&2
        echo "       Every updater would reject a payload whose .sig came from a" >&2
        echo "       different build." >&2
        return 1
    fi
    return 0
}

# release_notarize_resumable <path> <expected-commit> — the resume gate.
# Zero when the handle may be picked up: the state parses, carries every required
# field, names a known stage, its submitted artifact still exists and still hashes
# to what was submitted, the updater trio it was paired with is unchanged, and
# <expected-commit> is the recorded source_commit. Otherwise prints exactly why to
# stderr and returns non-zero — callers surface that reason either as a hard
# failure (an explicit --resume-notarize) or as a note before rebuilding (the
# automatic detection in a build-grade run).
release_notarize_resumable() {
    local path="$1" expected="$2" artifact recorded_sha actual_sha recorded_commit id stage

    [ -n "$path" ] || { echo "ERROR: release_notarize_resumable needs a state-file path" >&2; return 1; }
    [ -f "$path" ] || { echo "ERROR: no notarize state at $path" >&2; return 1; }
    [ -n "$expected" ] \
        || { echo "ERROR: release_notarize_resumable needs the expected source commit" >&2; return 1; }

    # A handle from before the two-stage paired shape (2026-08-02) is refused BY
    # NAME, never read as "no pairing recorded, so nothing can mismatch". Such a
    # handle also predates the updater repack, so its tarball on disk is the
    # ad-hoc one the staging gate would refuse anyway; saying so here turns a
    # confusing second failure into one clear instruction.
    # shellcheck disable=SC2086 # the field list is deliberately word-split
    if ! release_notarize_has_fields "$path" $RELEASE_NOTARIZE_REQUIRED_FIELDS; then
        echo "ERROR: $path predates the paired-set notarize handle and cannot be resumed." >&2
        echo "       It records no updater-payload pairing, so there is no way to tell" >&2
        echo "       whether the .app.tar.gz on disk belongs to the build that was" >&2
        echo "       submitted. Delete it and rebuild:" >&2
        echo "           rm $path" >&2
        return 1
    fi

    stage="$(release_notarize_field "$path" stage)" || return 1
    case "$stage" in
        "$RELEASE_NOTARIZE_STAGE_APP"|"$RELEASE_NOTARIZE_STAGE_DMG") ;;
        *) echo "ERROR: notarize state $path records an unknown stage '$stage'" >&2; return 1 ;;
    esac

    id="$(release_notarize_field "$path" submission_id)" || return 1
    [ -n "$id" ] || { echo "ERROR: notarize state $path records no submission id" >&2; return 1; }

    artifact="$(release_notarize_field "$path" artifact_path)" || return 1
    if [ -z "$artifact" ] || [ ! -f "$artifact" ]; then
        echo "ERROR: the submitted artifact is gone: '$artifact'" >&2
        echo "       (recorded by $path for $stage submission $id)" >&2
        return 1
    fi

    recorded_sha="$(release_notarize_field "$path" artifact_sha256)" || return 1
    actual_sha="$(release_staging_sha256 "$artifact")" \
        || { echo "ERROR: could not hash $artifact" >&2; return 1; }
    if [ "$actual_sha" != "$recorded_sha" ]; then
        echo "ERROR: checksum mismatch: $artifact is not what was submitted." >&2
        echo "       submitted: $recorded_sha" >&2
        echo "       on disk:   $actual_sha" >&2
        echo "       Apple scanned different bytes; refusing to staple/stage these." >&2
        return 1
    fi

    # The pairing. Checked here as well as before stapling, so a build-grade run
    # that would resume into an unpairable set rebuilds instead.
    release_notarize_updater_intact "$path" || return 1

    recorded_commit="$(release_notarize_field "$path" source_commit)" || return 1
    if [ "$recorded_commit" != "$expected" ]; then
        echo "ERROR: source-commit mismatch: the tree moved since the artifact was built." >&2
        echo "       built from: $recorded_commit" >&2
        echo "       tree is at: $expected" >&2
        echo "       Resuming would stamp a staging manifest with a commit the artifact" >&2
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
