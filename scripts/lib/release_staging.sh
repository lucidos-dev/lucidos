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
#     "platform_key": "darwin-aarch64",        (REQUIRED; see below)
#     "build_fingerprint": "v1:<hex>",         (optional; see below)
#     "recipe_fingerprint": "v1:<hex>",        (optional; see below)
#     "notarized": true|false,                 (optional; see below)
#     "artifacts": [ { "name": "<basename>", "sha256": "<hex>" }, … ] }
#
# `platform_key` is the `latest.json` `platforms` key the updater looks itself up
# under, and it describes the ARTIFACT (F10 in
# docs/audits/2026-08-02-macos-update-path-audit.md). It used to be derived at
# UPLOAD time from `uname -m`, which describes the host doing the upload, and a
# mislabelled key fails silently: an updater whose target key is absent from
# `platforms` reports "no update" rather than an error. Recording it here is what
# also makes `--release-attach` honest, since that path deliberately has no `.app`
# on disk to interrogate. Unlike the three optional keys, this one is REQUIRED:
# `release_staging_verify` refuses a manifest without it.
#
# `notarized` records whether the staged DMG carries a stapled Apple ticket. It
# is false only for a DEFERRED-DMG release (build-dmg.sh --defer-notarization),
# which stages the signed-but-unstapled DMG while the submission is still with
# Apple so the release can publish without waiting hours for a verdict. Every
# consumer that goes public derives its behaviour from THIS field rather than
# from a flag of its own, so a pending DMG cannot be published without its
# "notarization pending" banner. An ABSENT key means notarized: the only writer
# that omits it predates the deferred mode, and that writer stages solely after
# an Accepted verdict.
#
# The two fingerprints are the compiled-input gate (scripts/lib/
# release_build_fingerprint.sh). They record what the STAGED artifact was built
# from, in content terms rather than commit terms, so a later re-fold can ask
# "would rebuilding actually change any shipped byte?" and skip a redundant
# Apple notarization submission when the answer is no. They are OPTIONAL: a
# manifest written before the gate existed simply has neither, and the gate
# fails closed (rebuilds) rather than treating "absent" as "unchanged".
#
# Pure shell + python3 (hashlib for sha256, json for the manifest) — no git, gh,
# or network — so the unit tests (scripts/lib/release_staging_test.sh) exercise it
# fully offline. The "artifacts" are the staged FILES; this lib never builds or
# signs anything. It carries nothing machine-specific, so it is NOT stripped from
# the public mirror (build-dmg.sh's --release-build is a legitimate public path);
# contrast release_signing.sh / release_events.sh, which ARE in EXCLUDE_PATHS.

# release_staging_platform_key_for_binary <mach-o>: print the latest.json
# `platforms` key the given Mach-O is for, derived from the FILE with `lipo
# -archs`. This is the honest source: the staged app binary is the thing an
# updater will actually run, and it is the same binary whether the run that
# uploads it is on the build host or somewhere else entirely.
#
# A UNIVERSAL binary is a hard error rather than two keys, and rather than
# silently taking the first arch. The rest of the bundle is single-arch by
# construction: `stage_runtime_fetch_postgres` resolves ONE relocatable Postgres
# per target triple, and every other Mach-O under RESOURCE_NAMES is built for one
# triple too. So a fat `lucidos-app` inside a thin bundle would let us advertise
# `darwin-x86_64` for a payload whose bundled Postgres is arm64-only, which is a
# worse outcome than refusing. Making the whole bundle universal, or shipping two
# single-arch releases, is the real answer and the message says so.
release_staging_platform_key_for_binary() {
    local binary="$1" archs
    [ -f "$binary" ] || {
        echo "ERROR: cannot derive the platform key: no such binary: $binary" >&2
        return 1
    }
    command -v lipo >/dev/null 2>&1 || {
        echo "ERROR: cannot derive the platform key: lipo is not on PATH (Xcode CLT)" >&2
        return 1
    }
    archs="$(lipo -archs "$binary" 2>/dev/null | tr -s '[:space:]' ' ' | sed 's/^ //;s/ $//')" || archs=""
    [ -n "$archs" ] || {
        echo "ERROR: lipo could not read an architecture out of $binary" >&2
        return 1
    }
    case "$archs" in
        arm64|arm64e)   printf 'darwin-aarch64' ;;
        x86_64)         printf 'darwin-x86_64' ;;
        *\ *)
            echo "ERROR: $binary is a universal binary ($archs)." >&2
            echo "       Refusing to guess a single latest.json platform key for it, and refusing" >&2
            echo "       to advertise both: the rest of the bundle is single-arch by construction" >&2
            echo "       (one relocatable Postgres per target triple), so a second key would" >&2
            echo "       promise an update whose bundled Postgres is for the other architecture." >&2
            echo "       Ship one single-arch release per architecture, or make the WHOLE bundle" >&2
            echo "       universal first and then teach this function to emit both keys." >&2
            return 1
            ;;
        *)
            echo "ERROR: unsupported architecture '$archs' in $binary" >&2
            return 1
            ;;
    esac
}

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
#
# The platform key, the build/recipe fingerprints and the notarized flag are
# passed through the environment (RELEASE_STAGING_PLATFORM_KEY /
# RELEASE_STAGING_BUILD_FINGERPRINT / RELEASE_STAGING_RECIPE_FINGERPRINT /
# RELEASE_STAGING_NOTARIZED) rather than as positional arguments: the trailing
# <name…> is variadic, so a positional addition would be ambiguous, and every
# existing caller (including three test suites) keeps working untouched.
# Empty/unset means "not recorded".
#
# RELEASE_STAGING_PLATFORM_KEY is the one whose absence is not survivable: a
# manifest without it cannot say which `latest.json` key its payload belongs
# under, so `release_staging_verify` refuses one. It is still written the same
# absent-when-empty way as the rest, because "absent" is what lets that refusal
# tell a pre-F10 manifest apart from a writer that recorded an empty string.
#
# RELEASE_STAGING_NOTARIZED takes "true" or "false"; anything else (including
# unset) omits the key, which reads back as notarized. Both real writers set it
# explicitly — and a RESTAMP of an existing manifest must carry the old value
# forward, or a deferred staging would silently launder itself into a notarized
# one. release.sh's restage_manifest_for_commit does exactly that.
release_staging_write_manifest() {
    local dir="$1" version="$2" commit="$3"
    shift 3
    [ -d "$dir" ] || { echo "ERROR: staging dir '$dir' does not exist" >&2; return 1; }
    [ "$#" -ge 1 ] || { echo "ERROR: release_staging_write_manifest needs at least one artifact name" >&2; return 1; }
    local name
    for name in "$@"; do
        [ -f "$dir/$name" ] || { echo "ERROR: staged artifact '$dir/$name' is missing" >&2; return 1; }
    done
    DIR="$dir" VERSION="$version" COMMIT="$commit" \
    PLATFORM_KEY="${RELEASE_STAGING_PLATFORM_KEY:-}" \
    BUILD_FP="${RELEASE_STAGING_BUILD_FINGERPRINT:-}" \
    RECIPE_FP="${RELEASE_STAGING_RECIPE_FINGERPRINT:-}" \
    NOTARIZED="${RELEASE_STAGING_NOTARIZED:-}" \
    python3 - "$@" <<'PY'
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
}
# Only emit these keys when they were actually computed. An empty string in the
# manifest would be indistinguishable from a real value to a careless reader, and
# "absent" is the honest encoding of "not recorded". For platform_key the
# distinction is load-bearing rather than tidy: verify tells the two apart.
for key, env in (
    ("platform_key", "PLATFORM_KEY"),
    ("build_fingerprint", "BUILD_FP"),
    ("recipe_fingerprint", "RECIPE_FP"),
):
    value = os.environ.get(env, "")
    if value:
        manifest[key] = value
# Same rule for the notarization flag, but it is a BOOLEAN, so only the two
# literal spellings are honoured — a typo must not read back as a valid state.
notarized = os.environ.get("NOTARIZED", "")
if notarized in ("true", "false"):
    manifest["notarized"] = notarized == "true"
manifest["artifacts"] = artifacts
with open(os.path.join(dir_, "manifest.json"), "w", encoding="utf-8") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
PY
}

# release_staging_manifest_field <dir> <field> [--optional] — print the field.
# Non-zero if the manifest is missing or the field is absent. With --optional an
# absent field prints nothing and returns 0 — used for the fingerprint keys,
# which are legitimately missing on any staging written before the gate existed.
release_staging_manifest_field() {
    local dir="$1" field="$2" optional="${3:-}"
    [ -f "$dir/manifest.json" ] || { echo "ERROR: no manifest.json in '$dir'" >&2; return 1; }
    FIELD="$field" OPTIONAL="$optional" python3 - "$dir/manifest.json" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as f:
    manifest = json.load(f)
field = os.environ["FIELD"]
if field not in manifest:
    if os.environ.get("OPTIONAL") == "--optional":
        print("")
        sys.exit(0)
    sys.stderr.write("ERROR: manifest.json has no '%s' field\n" % field)
    sys.exit(1)
value = manifest[field]
# Render a JSON boolean as JSON spells it. Python's str(True) is "True", which
# no shell caller comparing against "true"/"false" would match — and the
# notarized flag is read by exactly such a comparison.
if isinstance(value, bool):
    value = "true" if value else "false"
print(value)
PY
}

# release_staging_is_notarized <dir> — zero when the staged DMG carries a stapled
# Apple ticket, non-zero when it does not (a deferred-DMG release). This is the
# single question every public-facing consumer asks: release-to-lucidos.sh
# composes the "notarization pending" banner from it, and release.sh decides
# whether the publish leaves the attach inputs in place.
#
# Fail-closed in the degenerate direction: a missing manifest reads as NOT
# notarized, so a corrupt staging errs toward the banner rather than toward a
# silent unnotarized publish. (Callers run release_staging_verify first anyway,
# which refuses a missing manifest outright.) The manifest-file test is done in
# THIS shell so the caller's errexit/ERR trap never sees a non-zero status from
# inside the command substitution.
release_staging_is_notarized() {
    local dir="$1" value
    [ -f "$dir/manifest.json" ] || return 1
    value="$(release_staging_manifest_field "$dir" notarized --optional 2>/dev/null || true)"
    [ "$value" != "false" ]
}

# release_staging_platform_key <dir>: print the recorded latest.json platform key.
# Non-zero when the manifest does not carry one, which is the same refusal
# release_staging_verify gives and is worded the same way. Callers that are about
# to BUILD a latest.json use this rather than a bare manifest_field read, so a
# manifest predating the recording can never reach the generator as an empty
# string.
release_staging_platform_key() {
    local dir="$1" value
    value="$(release_staging_manifest_field "$dir" platform_key --optional 2>/dev/null || true)"
    if [ -z "$value" ]; then
        echo "ERROR: no platform_key in $dir/manifest.json." >&2
        echo "       This staging predates platform-key recording, so nothing on disk says" >&2
        echo "       which latest.json platforms key its updater payload belongs under." >&2
        echo "       Re-stage the build (build-dmg.sh --release-build) rather than guessing." >&2
        return 1
    fi
    printf '%s' "$value"
}

# release_staging_verify <dir>: confirm the manifest is present, records a
# platform key, every listed artifact exists, and each artifact's recomputed
# sha256 matches the manifest. Prints a clear error to stderr and returns non-zero
# on the first problem. This is the integrity half of the guard (build-dmg.sh
# --release-attach runs it before any upload; release.sh --publish-verified runs
# it before going public).
#
# The platform-key half is an IDENTITY check in the spirit of
# release_staging_assert_commit, not an integrity one, and it lives here because
# this is the function every path already runs before publishing. Two refusals,
# deliberately distinct (the precedent is release_notarize_resumable's treatment
# of a pre-pairing handle): an ABSENT key means a manifest written before the key
# existed, which is re-stageable; a PRESENT but empty or malformed one means a
# writer that recorded nothing, which is a bug in this pipeline rather than an old
# artifact. Collapsing them would send an operator to re-stage over a bug a
# re-stage cannot fix.
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

# The platform key (F10). Absent and empty are told apart on purpose: only one of
# them is fixable by re-staging, and the other is a bug in the writer.
if "platform_key" not in manifest:
    sys.stderr.write(
        "ERROR: staging manifest records no platform_key\n"
        "       This manifest predates platform-key recording, so nothing in it says which\n"
        "       latest.json platforms key its updater payload belongs under, and the upload\n"
        "       would have to fall back to guessing from the upload host's architecture.\n"
        "       Re-stage the build: build-dmg.sh --release-build\n")
    sys.exit(1)
platform_key = manifest["platform_key"]
if not isinstance(platform_key, str) or not platform_key.strip():
    sys.stderr.write(
        "ERROR: staging manifest carries an empty platform_key (%r)\n"
        "       This is NOT an old manifest: something wrote the key and recorded nothing in\n"
        "       it. Re-staging would reproduce it. Fix the writer\n"
        "       (release_staging_platform_key_for_binary) first.\n" % (platform_key,))
    sys.exit(1)

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
