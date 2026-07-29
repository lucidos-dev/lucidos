#!/bin/bash
# Tests for the lightweight packaging contract in scripts/build-dmg.sh.
# Run: ./scripts/lib/build_dmg_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

echo "test: build-dmg resource contract includes packaged gateway stack"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --check 2>&1)"
rc=$?

if [ $rc -eq 0 ]; then
    pass "--check exits 0"
else
    fail "--check exited $rc; output: $out"
fi

for name in lucidos-gateway lucidos-engine frontend postgres sdk; do
    if echo "$out" | grep -q "$name"; then
        pass "mentions $name"
    else
        fail "missing $name from --check output: $out"
    fi
done

echo ""
echo "test: --release version-stamp guard rejects a release-version != RELEASE"
# The guard runs right after arg parsing — before the Darwin/tooling checks and
# the build — so this exits fast and never starts a build. CURRENT_STEP is unset
# at that point, so no event is emitted (no engine round-trip).
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release --release-version 99.99.99-build-dmg-test 2>&1)"
rc=$?
if [ $rc -ne 0 ]; then
    pass "--release with mismatched --release-version exits non-zero"
else
    fail "expected non-zero exit for version mismatch; got rc=$rc"
fi
case "$out" in
    *"version-stamp mismatch"*) pass "reports a version-stamp mismatch" ;;
    *) fail "missing version-stamp mismatch message; got: $out" ;;
esac

echo ""
echo "test: unknown argument is rejected"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --bogus-flag 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "unknown argument"; then
    pass "unknown argument exits non-zero with a clear message"
else
    fail "expected unknown-argument rejection; got rc=$rc out: $out"
fi

echo ""
echo "test: --release-build is recognized and shares the version-stamp guard"
# --release-build is a BUILD mode, so it runs the same up-front version guard as
# --release and exits fast (before any build) on a mismatch — proving it parses.
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release-build --release-version 99.99.99-build-dmg-test 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "version-stamp mismatch"; then
    pass "--release-build rejects a mismatched --release-version (recognized as a build mode)"
else
    fail "expected version-stamp mismatch for --release-build; got rc=$rc out: $out"
fi

echo ""
echo "test: --release-attach requires --staging-dir"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release-attach --upload-tag v9.9.9 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "requires --staging-dir"; then
    pass "--release-attach without --staging-dir exits non-zero with a clear message"
else
    fail "expected --staging-dir requirement; got rc=$rc out: $out"
fi

# ── --release-attach staging guard (offline) ─────────────────────────────────
# Build a staging fixture (fake artifacts + a real manifest) and corrupt it. Each
# case below fails at staging VERIFICATION — before any gh/network/event — so the
# whole suite stays offline + signing-free.
# shellcheck source=scripts/lib/release_staging.sh
source "$PROJECT_DIR/scripts/lib/release_staging.sh"
make_staging() {
    local dir; dir="$(mktemp -d)"
    printf 'dmg\n' > "$dir/Lucidos_0.0.0_aarch64.dmg"
    printf 'tar\n' > "$dir/Lucidos.app.tar.gz"
    printf 'sig\n' > "$dir/Lucidos.app.tar.gz.sig"
    release_staging_write_manifest "$dir" 0.0.0 abc123 \
        Lucidos_0.0.0_aarch64.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig >/dev/null
    printf '%s' "$dir"
}

echo ""
echo "test: --release-attach refuses a staging dir with no manifest"
EMPTY="$(mktemp -d)"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release-attach --staging-dir "$EMPTY" --upload-tag v9.9.9 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "manifest"; then
    pass "missing manifest is refused"
else
    fail "expected missing-manifest refusal; got rc=$rc out: $out"
fi
rm -rf "$EMPTY"

echo ""
echo "test: --release-attach refuses a missing staged artifact"
S="$(make_staging)"
rm -f "$S/Lucidos.app.tar.gz.sig"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release-attach --staging-dir "$S" --upload-tag v9.9.9 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "missing"; then
    pass "missing artifact is refused"
else
    fail "expected missing-artifact refusal; got rc=$rc out: $out"
fi
rm -rf "$S"

echo ""
echo "test: --release-attach refuses a checksum-mismatched staged artifact"
S="$(make_staging)"
printf 'tampered\n' >> "$S/Lucidos_0.0.0_aarch64.dmg"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release-attach --staging-dir "$S" --upload-tag v9.9.9 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "checksum mismatch"; then
    pass "checksum mismatch is refused"
else
    fail "expected checksum-mismatch refusal; got rc=$rc out: $out"
fi
rm -rf "$S"

echo ""
echo "test: --emit-tarball is a recognized flag (not 'unknown argument')"
# Compose it with a build mode + a mismatched --release-version so it hits the
# fast version-stamp guard BEFORE any build — proving the flag parsed. A genuine
# emit needs a full macOS build, which this offline suite never runs.
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --emit-tarball --release-build --release-version 99.99.99-build-dmg-test 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "version-stamp mismatch" && ! echo "$out" | grep -q "unknown argument"; then
    pass "--emit-tarball parses and composes with a build mode"
else
    fail "expected --emit-tarball to parse and hit the version guard; got rc=$rc out: $out"
fi

echo ""
echo "test: --emit-tarball is documented in --help"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --help 2>&1)"
if echo "$out" | grep -q -- "--emit-tarball"; then
    pass "--help documents --emit-tarball"
else
    fail "--help does not mention --emit-tarball: $out"
fi

echo ""
echo "test: headless-tarball lib + its test ship alongside build-dmg.sh"
for f in scripts/lib/headless_tarball.sh scripts/lib/headless_tarball_test.sh; do
    if [ -f "$PROJECT_DIR/$f" ]; then
        pass "$f exists"
    else
        fail "$f is missing"
    fi
done

echo ""
echo "test: notarize-resume lib + its test ship alongside build-dmg.sh"
for f in scripts/lib/release_notarize.sh scripts/lib/release_notarize_test.sh; do
    if [ -f "$PROJECT_DIR/$f" ]; then
        pass "$f exists"
    else
        fail "$f is missing"
    fi
done

# ── resumable notarization (offline) ─────────────────────────────────────────
# The notarize stage must survive losing the process waiting on Apple: it submits
# with --no-wait, persists the submission id, and polls. Everything below stops
# BEFORE any notarytool call, so the suite stays offline.
#
# Apple credentials are genuinely present in a release-capable shell (the engine
# injects them), so every invocation here runs with them UNSET — otherwise a
# resume that passes the gate would go on to really ask Apple about a fake
# submission. run_notarize_resume checks credentials right after the gate, which
# is what makes "gate passed" observable without a network call.
no_apple_creds() {
    env -u APPLE_ID -u APPLE_PASSWORD -u APPLE_TEAM_ID -u APPLE_SIGNING_IDENTITY \
        -u APPLE_API_KEY_PATH -u APPLE_API_KEY_ID -u APPLE_API_ISSUER_ID "$@"
}

# shellcheck source=scripts/lib/release_notarize.sh
source "$PROJECT_DIR/scripts/lib/release_notarize.sh"
RELEASE_VERSION_UNDER_TEST="$(tr -d '[:space:]' < "$PROJECT_DIR/RELEASE")"
NOTARIZE_STATE="$(release_notarize_state_path "$PROJECT_DIR" "$RELEASE_VERSION_UNDER_TEST")"
# A real release may have a live handle for this version in this very tree —
# never clobber it. Stash it for the duration and put it back at the end.
NOTARIZE_STATE_BACKUP=""
if [ -f "$NOTARIZE_STATE" ]; then
    NOTARIZE_STATE_BACKUP="$(mktemp)"
    cp "$NOTARIZE_STATE" "$NOTARIZE_STATE_BACKUP"
fi
restore_notarize_state() {
    rm -f "$NOTARIZE_STATE" "${FAKE_DMG:-}"
    if [ -n "$NOTARIZE_STATE_BACKUP" ]; then
        mv "$NOTARIZE_STATE_BACKUP" "$NOTARIZE_STATE"
    fi
}
trap restore_notarize_state EXIT

FAKE_SUBMISSION="ca4778bf-bf0e-4c04-9f2c-7459885cdb51"
HEAD_COMMIT="$(git -C "$PROJECT_DIR" rev-parse HEAD)"

# A resume handle must point at a DMG inside THIS tree's bundle dir (the resume
# pairs it with the .app.tar.gz + .sig found there, so the two must come from the
# same build). The fixture therefore plants its fake DMG where a real one would
# live — and bails out entirely if a real build is already sitting there, rather
# than disturbing it.
BUNDLE_DMG_DIR="$PROJECT_DIR/target/release/bundle/dmg"
FAKE_DMG=""
REAL_BUILD_PRESENT=0
if [ -n "$(/usr/bin/find "$BUNDLE_DMG_DIR" -maxdepth 1 -name '*.dmg' 2>/dev/null | head -1)" ]; then
    REAL_BUILD_PRESENT=1
fi

seed_notarize_state() {  # <source-commit>
    mkdir -p "$BUNDLE_DMG_DIR"
    FAKE_DMG="$BUNDLE_DMG_DIR/Lucidos_${RELEASE_VERSION_UNDER_TEST}_test.dmg"
    printf 'pretend signed dmg\n' > "$FAKE_DMG"
    release_notarize_write_state "$NOTARIZE_STATE" "$FAKE_SUBMISSION" "$FAKE_DMG" \
        "$RELEASE_VERSION_UNDER_TEST" "$(release_staging_sha256 "$FAKE_DMG")" \
        "$1" "2026-07-28T08:22:00Z"
}
clear_notarize_state() {
    rm -f "$NOTARIZE_STATE" "${FAKE_DMG:-}"
    FAKE_DMG=""
}

echo ""
echo "test: --resume-notarize without a handle refuses instead of rebuilding"
rm -f "$NOTARIZE_STATE"
out="$(no_apple_creds "$PROJECT_DIR/scripts/build-dmg.sh" --resume-notarize 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "no notarize state"; then
    pass "a missing resume handle is refused with a clear message"
else
    fail "expected a missing-handle refusal; got rc=$rc out: $out"
fi

echo ""
echo "test: --resume-notarize accepts an intact handle (gate passes, then needs creds)"
if [ "$REAL_BUILD_PRESENT" = "1" ]; then
    echo "  skip: a real build sits in $BUNDLE_DMG_DIR (refusing to disturb it)"
else
seed_notarize_state "$HEAD_COMMIT"
out="$(no_apple_creds "$PROJECT_DIR/scripts/build-dmg.sh" --resume-notarize 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "$FAKE_SUBMISSION" \
   && echo "$out" | grep -qi "needs APPLE_ID"; then
    pass "an intact handle passes the gate and reaches the credential check"
else
    fail "expected the gate to pass for an intact handle; got rc=$rc out: $out"
fi
if echo "$out" | grep -qi "checksum\|mismatch"; then
    fail "an intact handle should not report any mismatch: $out"
else
    pass "no spurious mismatch for an intact handle"
fi
clear_notarize_state
fi

echo ""
echo "test: --resume-notarize refuses a DMG whose checksum no longer matches"
# The load-bearing one: Apple scanned specific bytes. If the DMG on disk is not
# those bytes, stapling + staging it would ship something Apple never approved.
if [ "$REAL_BUILD_PRESENT" = "1" ]; then
    echo "  skip: a real build sits in $BUNDLE_DMG_DIR (refusing to disturb it)"
else
seed_notarize_state "$HEAD_COMMIT"
printf 'tampered\n' >> "$FAKE_DMG"
out="$(no_apple_creds "$PROJECT_DIR/scripts/build-dmg.sh" --resume-notarize 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "checksum mismatch" \
   && echo "$out" | grep -qi "cannot resume notarization"; then
    pass "a mismatched DMG checksum refuses to resume"
else
    fail "expected a checksum-mismatch refusal; got rc=$rc out: $out"
fi
clear_notarize_state
fi

echo ""
echo "test: --resume-notarize refuses when the tree moved off the built commit"
# Resuming stamps the staging manifest with the RESUMING run's HEAD, so a moved
# tree would make manifest.source_commit name a commit the DMG was never built
# from — and release.sh --publish-verified's identity guard would pass on a lie.
if [ "$REAL_BUILD_PRESENT" = "1" ]; then
    echo "  skip: a real build sits in $BUNDLE_DMG_DIR (refusing to disturb it)"
else
seed_notarize_state "0000000000000000000000000000000000000000"
out="$(no_apple_creds "$PROJECT_DIR/scripts/build-dmg.sh" --resume-notarize 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "source-commit mismatch"; then
    pass "a moved source commit refuses to resume"
else
    fail "expected a source-commit refusal; got rc=$rc out: $out"
fi
clear_notarize_state
fi

echo ""
echo "test: --resume-notarize refuses a handle pointing outside this tree's bundle"
# Staging pairs the recorded DMG with the .app.tar.gz + .sig found under the
# bundle dir, so a DMG from elsewhere could produce a manifest describing two
# different builds.
OUTSIDE_DMG="$(mktemp -t lucidos-outside)"
printf 'pretend signed dmg\n' > "$OUTSIDE_DMG"
release_notarize_write_state "$NOTARIZE_STATE" "$FAKE_SUBMISSION" "$OUTSIDE_DMG" \
    "$RELEASE_VERSION_UNDER_TEST" "$(release_staging_sha256 "$OUTSIDE_DMG")" \
    "$HEAD_COMMIT" "2026-07-28T08:22:00Z"
out="$(no_apple_creds "$PROJECT_DIR/scripts/build-dmg.sh" --resume-notarize 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "outside this tree's bundle dir"; then
    pass "a DMG outside the bundle dir refuses to resume"
else
    fail "expected an outside-bundle refusal; got rc=$rc out: $out"
fi
rm -f "$NOTARIZE_STATE" "$OUTSIDE_DMG"

echo ""
echo "test: --adopt-submission validates its argument"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --adopt-submission 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "requires a notary submission UUID"; then
    pass "--adopt-submission without an argument is refused"
else
    fail "expected a missing-argument refusal; got rc=$rc out: $out"
fi
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --adopt-submission not-a-uuid 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "expects a notary submission UUID"; then
    pass "--adopt-submission shape-checks the UUID before touching Apple"
else
    fail "expected a UUID shape refusal; got rc=$rc out: $out"
fi

echo ""
echo "test: --adopt-submission needs the submitted DMG still on disk"
if [ "$REAL_BUILD_PRESENT" = "1" ]; then
    echo "  skip: a real build sits in $BUNDLE_DMG_DIR (adoption would write a handle for it)"
else
    rm -f "$NOTARIZE_STATE"
    out="$(no_apple_creds "$PROJECT_DIR/scripts/build-dmg.sh" \
            --adopt-submission "$FAKE_SUBMISSION" 2>&1)"
    rc=$?
    if [ $rc -ne 0 ] && echo "$out" | grep -qi "no built .dmg"; then
        pass "adoption without an on-disk DMG is refused"
    else
        fail "expected a no-DMG refusal; got rc=$rc out: $out"
    fi
fi

echo ""
echo "test: --adopt-submission picks the real DMG, and refuses to guess between two"
if [ "$REAL_BUILD_PRESENT" = "1" ]; then
    echo "  skip: a real build sits in $BUNDLE_DMG_DIR (adoption would write a handle for it)"
else
    # A run killed mid-refresh can leave refresh_dmg_payload's .rw.dmg behind;
    # adopting THAT would record a checksum for bytes Apple never scanned.
    rm -f "$NOTARIZE_STATE"
    mkdir -p "$BUNDLE_DMG_DIR"
    REAL="$BUNDLE_DMG_DIR/Lucidos_${RELEASE_VERSION_UNDER_TEST}_aarch64.dmg"
    printf 'real\n' > "$REAL"
    printf 'intermediate\n' > "$BUNDLE_DMG_DIR/Lucidos_${RELEASE_VERSION_UNDER_TEST}_aarch64.rw.dmg"
    out="$(no_apple_creds "$PROJECT_DIR/scripts/build-dmg.sh" \
            --adopt-submission "$FAKE_SUBMISSION" 2>&1)"
    if echo "$out" | grep -q "Adopting in-flight submission .* for $(basename "$REAL")"; then
        pass "a leftover .rw.dmg is ignored in favour of the real DMG"
    else
        fail "expected adoption of $(basename "$REAL"); got: $out"
    fi
    if [ "$(release_notarize_field "$NOTARIZE_STATE" dmg_path 2>/dev/null)" = "$REAL" ]; then
        pass "the handle records the real DMG"
    else
        fail "the handle does not record $REAL"
    fi

    printf 'other\n' > "$BUNDLE_DMG_DIR/Lucidos_${RELEASE_VERSION_UNDER_TEST}_x86_64.dmg"
    rm -f "$NOTARIZE_STATE"
    out="$(no_apple_creds "$PROJECT_DIR/scripts/build-dmg.sh" \
            --adopt-submission "$FAKE_SUBMISSION" 2>&1)"
    rc=$?
    if [ $rc -ne 0 ] && echo "$out" | grep -qi "candidate DMGs"; then
        pass "two candidate DMGs are refused rather than guessed between"
    else
        fail "expected an ambiguity refusal; got rc=$rc out: $out"
    fi
    if [ -f "$NOTARIZE_STATE" ]; then
        fail "an ambiguous adoption still wrote a handle"
    else
        pass "no handle is written when adoption is ambiguous"
    fi
    rm -f "$BUNDLE_DMG_DIR"/*.dmg "$NOTARIZE_STATE"
fi

echo ""
echo "test: --resume-notarize / --adopt-submission are rejected with --release-attach"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --resume-notarize --release-attach --staging-dir /tmp 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "cannot be combined with --release-attach"; then
    pass "resume + attach is refused (attach has nothing left to notarize)"
else
    fail "expected a resume+attach refusal; got rc=$rc out: $out"
fi

echo ""
echo "test: --help documents the resume flags"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --help 2>&1)"
for flag in --resume-notarize --adopt-submission; do
    if echo "$out" | grep -q -- "$flag"; then
        pass "--help documents $flag"
    else
        fail "--help does not mention $flag"
    fi
done

echo ""
echo "test: the notary submit no longer holds a foreground --wait"
# The whole point: a wait that outlives the process can never complete, because
# the orchestration layer caps background tasks at 3600s.
BUILD_DMG="$PROJECT_DIR/scripts/build-dmg.sh"
if grep -q -- '--no-wait' "$BUILD_DMG"; then
    pass "submit uses --no-wait"
else
    fail "build-dmg.sh no longer passes --no-wait to notarytool submit"
fi
if grep -qE 'notarytool[^|]*--wait' "$BUILD_DMG"; then
    fail "build-dmg.sh still runs a foreground 'notarytool … --wait'"
else
    pass "no foreground 'notarytool … --wait' remains"
fi
if grep -q 'release_notarize_write_state' "$BUILD_DMG"; then
    pass "the submission id is persisted to a resume handle"
else
    fail "build-dmg.sh never writes a notarize resume handle"
fi

echo ""
echo "test: notarization credentials never reach argv, and no keychain profile"
# argv is world-readable (`ps -eo command`). The password must only ever be piped
# on stdin, and `notarytool store-credentials` cannot come back: the release runs
# headless, where the Security framework refuses with "User interaction is not
# allowed" and the whole release dies at the notarize step.
# Check the notarytool CALL SITES, not the whole file: the comment block above
# notarytool_run deliberately names `--password` and `store-credentials` to record
# why neither may come back, and a bare `-p` also spells `mkdir -p`. Credential
# flags live on backslash-continuation lines, so drop comments, fold each command
# onto one line, and only then keep the notarytool invocations.
NOTARYTOOL_CALLS="$(grep -vE '^[[:space:]]*#' "$BUILD_DMG" \
    | sed -e ':a' -e '/\\$/{N;s/\\\n//;ba' -e '}' \
    | grep 'notarytool' || true)"
if [ -n "$NOTARYTOOL_CALLS" ]; then
    pass "notarytool is still invoked from build-dmg.sh"
else
    fail "found no notarytool invocation in build-dmg.sh"
fi
if printf '%s\n' "$NOTARYTOOL_CALLS" | grep -q -- '--password'; then
    fail "build-dmg.sh passes --password on the notarytool command line"
else
    pass "no --password in argv"
fi
if printf '%s\n' "$NOTARYTOOL_CALLS" \
        | grep -qE 'store-credentials|--keychain-profile|(^|[[:space:]])-p([[:space:]]|$)'; then
    fail "build-dmg.sh reintroduced a notarytool keychain profile"
else
    pass "no keychain profile / store-credentials at any notarytool call site"
fi
if grep -q "printf '%s' \"\$APPLE_PASSWORD\" | xcrun notarytool" "$BUILD_DMG"; then
    pass "the app-specific password is piped on stdin"
else
    fail "build-dmg.sh no longer pipes APPLE_PASSWORD on stdin"
fi
# -i (issuer) is REQUIRED for Team API keys and must be OMITTED for Individual
# keys, so the conditional branch is load-bearing, not a style choice.
if printf '%s\n' "$NOTARYTOOL_CALLS" | grep -q -- "-i \"\$APPLE_API_ISSUER_ID\""; then
    pass "-i is still passed conditionally on APPLE_API_ISSUER_ID"
else
    fail "the conditional -i (issuer) handling for Team vs Individual keys is gone"
fi

echo ""
echo "test: release scripts keep the failure-emit contract (errtrace + ERR trap)"
# A failing stage must emit ReleaseStepFailed, not exit silently. That relies on
# `set -E` (so the ERR trap inherits into shell functions) AND an `on_err` ERR
# trap. Without `-E` the trap never fires for failures inside sign/refresh/upload
# functions and the cockpit stalls — guard against a future edit dropping either.
#
# release.sh + release-to-lucidos.sh are in the release EXCLUDE_PATHS and are
# absent from the public mirror, so skip (never fail) when they aren't there.
for s in build-dmg.sh release.sh release-to-lucidos.sh; do
    f="$PROJECT_DIR/scripts/$s"
    if [ ! -f "$f" ]; then
        echo "  skip: $s is not present (stripped from the public mirror)"
        continue
    fi
    if grep -q 'set -Eeuo pipefail' "$f"; then
        pass "$s sets errtrace (set -Eeuo pipefail)"
    else
        fail "$s missing 'set -Eeuo pipefail' (ERR trap won't fire inside functions)"
    fi
    if grep -q 'trap on_err ERR' "$f"; then
        pass "$s arms the on_err ERR trap"
    else
        fail "$s missing 'trap on_err ERR'"
    fi
done

echo ""
echo "test: release.sh exposes the notarize resume as its own phase"
RELEASE_SH="$PROJECT_DIR/scripts/release.sh"
if [ ! -f "$RELEASE_SH" ]; then
    echo "  skip: release.sh is not present (stripped from the public mirror)"
else
    out="$("$RELEASE_SH" 2>&1)"
    if echo "$out" | grep -q -- "--resume-notarize"; then
        pass "usage documents --resume-notarize"
    else
        fail "release.sh usage does not mention --resume-notarize: $out"
    fi

    out="$("$RELEASE_SH" --resume-notarize --publish-verified 9.9.9 2>&1)"
    rc=$?
    if [ $rc -ne 0 ] && echo "$out" | grep -qi "cannot combine"; then
        pass "--resume-notarize and --publish-verified are mutually exclusive"
    else
        fail "expected a phase-conflict refusal; got rc=$rc out: $out"
    fi

    # No worktree ⇒ nothing to resume. This refusal lands BEFORE the creds step,
    # so the check emits no ReleaseStep* event into the workspace.
    out="$("$RELEASE_SH" --resume-notarize 9.9.9 2>&1)"
    rc=$?
    if [ $rc -ne 0 ] && echo "$out" | grep -qi "No Phase A worktree"; then
        pass "resuming without a Phase A worktree is refused"
    else
        fail "expected a missing-worktree refusal; got rc=$rc out: $out"
    fi

    # A Phase A killed mid-wait leaves the handle but no verify-build state, so
    # re-running --verify-build must RESUME rather than demand -c and rebuild.
    #
    # Two containment measures on every invocation below, via `sandboxed`:
    #   • a minimal PATH, so the `lucidos` CLI is not found and
    #     release_events.sh no-ops — otherwise this test would emit phantom
    #     ReleaseStep* events for 9.9.9 into the developer's workspace;
    #   • no signing credentials (no_apple_creds, above, plus the Tauri updater
    #     key), so the resume always stops at the credential gate. On a box that
    #     has them exported — a release machine; every Lucidos-spawned subprocess
    #     inherits them — the resume would run on, and its first real step is now
    #     the release-candidate push, which would force-push an rc/9.9.9 branch
    #     to the PUBLIC mirror from a test run.
    sandboxed() {
        no_apple_creds env -u TAURI_SIGNING_PRIVATE_KEY -u TAURI_SIGNING_PRIVATE_KEY_PATH \
            PATH="/usr/bin:/bin:/usr/sbin:/sbin" "$@"
    }
    FAKE_WT="$PROJECT_DIR/.lucidos/release-worktrees/9.9.9"
    if [ -e "$FAKE_WT" ]; then
        echo "  skip: $FAKE_WT already exists (refusing to disturb a real release)"
    else
        mkdir -p "$FAKE_WT/.lucidos/release-state"
        # release.sh promotes only on a genuinely resumable handle, so the fixture
        # needs a real DMG, its real sha256, and the worktree's HEAD.
        WT_DMG="$(mktemp -t lucidos-wt-dmg)"
        printf 'pretend signed dmg\n' > "$WT_DMG"
        release_notarize_write_state \
            "$FAKE_WT/.lucidos/release-state/notarize-9.9.9.json" \
            "$FAKE_SUBMISSION" "$WT_DMG" 9.9.9 \
            "$(release_staging_sha256 "$WT_DMG")" \
            "$(git -C "$FAKE_WT" rev-parse HEAD)" "2026-07-28T08:22:00Z"
        out="$(sandboxed "$RELEASE_SH" --verify-build 9.9.9 2>&1)"
        if echo "$out" | grep -qi "found a resumable notarization"; then
            pass "--verify-build auto-promotes to a resume when a handle exists"
        else
            fail "expected --verify-build to auto-resume; got: $out"
        fi
        if echo "$out" | grep -qi "requires -c"; then
            fail "--verify-build still demanded -c on the resume path: $out"
        else
            pass "the resume path no longer demands -c (that changelog is committed)"
        fi
        if echo "$out" | grep -qi "Creating worktree\|Bumping RELEASE"; then
            fail "the resume path started a fresh Phase A instead of resuming: $out"
        else
            pass "no worktree creation / RELEASE bump on the resume path"
        fi

        # A handle that exists but is NOT resumable (DMG rebuilt, tree moved) must
        # NOT promote — otherwise --verify-build dead-ends on "cannot resume"
        # instead of doing the rebuild that was asked for.
        printf '{"submission_id":"%s","dmg_path":"/nonexistent/gone.dmg","version":"9.9.9","dmg_sha256":"deadbeef","source_commit":"abc","submitted_at":"t"}\n' \
            "$FAKE_SUBMISSION" > "$FAKE_WT/.lucidos/release-state/notarize-9.9.9.json"
        out="$(sandboxed "$RELEASE_SH" --verify-build 9.9.9 2>&1)"
        if echo "$out" | grep -qi "is NOT resumable"; then
            pass "a stale handle is reported as not resumable"
        else
            fail "expected a not-resumable notice; got: $out"
        fi
        if echo "$out" | grep -qi "Resuming it instead"; then
            fail "a stale handle was still promoted to the resume path: $out"
        else
            pass "a stale handle does not promote (the rebuild path stays reachable)"
        fi
        rm -rf "$FAKE_WT" "$WT_DMG"
    fi

    # Ordering invariant: build-dmg.sh drops the resume handle as soon as staging
    # succeeds, so the verify-build state must already be on disk by then. If this
    # write moved back after the build, a kill in that window would strand staged
    # artifacts that neither --resume-notarize nor --publish-verified can pick up.
    # There are exactly two of each call site — run_resume_notarize's, then Phase
    # A's — so comparing them pairwise in file order pins both blocks. The build
    # pattern keeps the closing quote of "$WORKTREE_DIR/scripts/build-dmg.sh" so it
    # matches the real invocations and not the command shown in an error message.
    WRITE_LINES="$(grep -n 'write_verify_build_state$' "$RELEASE_SH" | cut -d: -f1 | tr '\n' ' ')"
    BUILD_LINES="$(grep -n 'build-dmg.sh" --release-build' "$RELEASE_SH" | cut -d: -f1 | tr '\n' ' ')"
    for slot in 1:resume 2:phase-a; do
        idx="${slot%%:*}"; name="${slot#*:}"
        w="$(printf '%s' "$WRITE_LINES" | cut -d' ' -f"$idx")"
        b="$(printf '%s' "$BUILD_LINES" | cut -d' ' -f"$idx")"
        if [ -n "$w" ] && [ -n "$b" ] && [ "$w" -lt "$b" ]; then
            pass "$name writes the verify-build state before invoking build-dmg.sh"
        else
            fail "$name must write the verify-build state BEFORE the build (write=$w build=$b)"
        fi
    done
fi

# ── Deferred DMG (--defer-notarization) ──────────────────────────────────────
# The mode that lets a release publish without Apple's verdict. Its whole safety
# story is "an unnotarized DMG cannot ship without its banner", and that rests on
# three structural facts asserted here: the flag is refused on any path that
# would upload in the same process (where no banner can be composed), the state
# travels in the staging manifest, and the deferred path keeps the resume handle
# the attach step needs.
#
# All three refusals run before the Darwin/tooling checks and before the version
# is resolved, so they exit fast and offline — no build, no RELEASE file needed.
echo ""
echo "test: --defer-notarization is refused wherever it could ship unbannered"

out="$("$BUILD_DMG" --release --defer-notarization 2>&1)"; rc=$?
if [ $rc -ne 0 ]; then pass "--release --defer-notarization exits non-zero"; else fail "one-shot --release accepted --defer-notarization (rc=$rc)"; fi
case "$out" in
    *"two-phase"*) pass "names the two-phase flow as the way to defer" ;;
    *) fail "refusal did not point at the two-phase flow: $out" ;;
esac

out="$("$BUILD_DMG" --release-attach --defer-notarization --staging-dir /nonexistent --upload-tag v9.9.9 2>&1)"; rc=$?
if [ $rc -ne 0 ]; then pass "--release-attach --defer-notarization exits non-zero"; else fail "attach accepted --defer-notarization (rc=$rc)"; fi

out="$("$BUILD_DMG" --defer-notarization 2>&1)"; rc=$?
if [ $rc -ne 0 ]; then pass "--defer-notarization without a build exits non-zero"; else fail "a build-less --defer-notarization was accepted (rc=$rc)"; fi
case "$out" in
    *"nothing to defer"*) pass "says there is nothing to defer without a build" ;;
    *) fail "unexpected refusal for a build-less defer: $out" ;;
esac

# Refusing the flag COMBINATION is not enough: nothing stopped a separate later
# `--release-attach` aimed at a deferred staging dir, which would upload an
# unstapled DMG with no banner and no dmg_pending on LucidosReleased. The guard
# therefore lives on the MANIFEST, and only release-to-lucidos.sh — after
# composing the banner — may override it.
echo ""
echo "test: a bare --release-attach cannot ship a pending staging"
S="$(mktemp -d)"
printf 'dmg\n' > "$S/Lucidos_0.0.0_aarch64.dmg"
printf 'tar\n' > "$S/Lucidos.app.tar.gz"
printf 'sig\n' > "$S/Lucidos.app.tar.gz.sig"
RELEASE_STAGING_NOTARIZED=false release_staging_write_manifest "$S" 0.0.0 abc123 \
    Lucidos_0.0.0_aarch64.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig >/dev/null
out="$("$BUILD_DMG" --release-attach --staging-dir "$S" --upload-tag v9.9.9 2>&1)"; rc=$?
if [ $rc -ne 0 ]; then pass "a pending staging is refused by a bare --release-attach"; else fail "a bare attach uploaded a NOT-notarized DMG (rc=$rc)"; fi
case "$out" in
    *"NOT-notarized"*) pass "the refusal names the reason" ;;
    *) fail "unexpected refusal text: $out" ;;
esac
case "$out" in
    *"--publish-verified"*) pass "it points at the two-phase flow that adds the banner" ;;
    *) fail "the refusal does not say how to publish a deferred release" ;;
esac
# release-to-lucidos.sh's override must exist AND be reachable only on the
# banner branch — otherwise the guard above is decorative.
if grep -q -- '--allow-pending-notarization' "$PROJECT_DIR/scripts/release-to-lucidos.sh" 2>/dev/null; then
    pass "release-to-lucidos.sh carries the banner-backed override"
else
    # release-to-lucidos.sh is stripped from the mirror; only assert when present.
    if [ -f "$PROJECT_DIR/scripts/release-to-lucidos.sh" ]; then
        fail "the deferred publish has no way past the pending-attach guard"
    else
        pass "skip: release-to-lucidos.sh is stripped from the public mirror"
    fi
fi
rm -rf "$S"

echo ""
echo "test: the deferred path records its state and keeps the attach step's inputs"
# shellcheck disable=SC2016 # matching the literal source text, not expanding it
if grep -q 'RELEASE_STAGING_NOTARIZED="$DMG_NOTARIZED_STATE"' "$BUILD_DMG"; then
    pass "staging writes the notarization state into the manifest"
else
    fail "stage_release_artifacts does not pass RELEASE_STAGING_NOTARIZED — the banner would have nothing to key off"
fi

# The single most dangerous regression: clearing the resume handle on a deferred
# run strands a PUBLISHED DMG with no way to find its submission, and the only
# recovery is the rebuild this feature exists to avoid.
FINALIZE="$(awk '/^finalize_release_artifacts\(\) \{/,/^\}/' "$BUILD_DMG")"
# shellcheck disable=SC2016 # matching the literal source text, not expanding it
case "$FINALIZE" in
    *'if [ "$DMG_NOTARIZED_STATE" = "false" ]'*) pass "finalize guards its cleanup on the notarization state" ;;
    *) fail "finalize_release_artifacts clears the handle unconditionally — a deferred release could not be finished" ;;
esac
CLEAR_LINE="$(printf '%s\n' "$FINALIZE" | grep -n 'release_notarize_clear' | cut -d: -f1 | head -1)"
GUARD_LINE="$(printf '%s\n' "$FINALIZE" | grep -n 'DMG_NOTARIZED_STATE' | cut -d: -f1 | head -1)"
if [ -n "$CLEAR_LINE" ] && [ -n "$GUARD_LINE" ] && [ "$GUARD_LINE" -lt "$CLEAR_LINE" ]; then
    pass "the guard precedes release_notarize_clear"
else
    fail "release_notarize_clear is not behind the deferred guard (guard=$GUARD_LINE clear=$CLEAR_LINE)"
fi

# A deferred resume must not poll: the point is to stage what is already built
# while the submission stays in flight.
RESUME="$(awk '/^run_notarize_resume\(\) \{/,/^\}/' "$BUILD_DMG")"
DEFER_LINE="$(printf '%s\n' "$RESUME" | grep -n 'DEFER_NOTARIZATION' | cut -d: -f1 | head -1)"
AWAIT_LINE="$(printf '%s\n' "$RESUME" | grep -n 'notarize_await_verdict' | cut -d: -f1 | head -1)"
if [ -n "$DEFER_LINE" ] && [ -n "$AWAIT_LINE" ] && [ "$DEFER_LINE" -lt "$AWAIT_LINE" ]; then
    pass "a deferred resume returns before notarize_await_verdict"
else
    fail "the deferred branch does not short-circuit the poll (defer=$DEFER_LINE await=$AWAIT_LINE)"
fi

if grep -q -- '--defer-notarization' "$BUILD_DMG" && "$BUILD_DMG" --help 2>&1 | grep -q -- '--defer-notarization'; then
    pass "usage documents --defer-notarization"
else
    fail "--defer-notarization is undocumented in usage"
fi

echo ""
# release.sh, release-to-lucidos.sh and release_notes.sh are all stripped from
# the public mirror (RELEASE_TREE_EXCLUDE_PATHS), but THIS test file ships. Guard
# the assertions about them exactly as the resume block above does, or a
# contributor running the suite from a clone gets a run of failures — plus one
# vacuous pass, since `grep -q` on a missing file returns non-zero, which would
# read as "the duplicate awk is gone".
if [ ! -f "$RELEASE_SH" ]; then
    echo "  skip: release.sh is not present (stripped from the public mirror)"
else
    echo ""
    echo "test: release.sh wires the deferred mode through Phase A without laundering it"
    if grep -q -- '--defer-notarization' "$RELEASE_SH"; then
        pass "release.sh exposes --defer-notarization"
    else
        fail "release.sh does not expose --defer-notarization"
    fi
    # A restamp that dropped the flag would turn a pending staging into one that
    # reads as notarized — publishing an unstapled DMG with clean notes.
    RESTAMP="$(awk '/^restage_manifest_for_commit\(\) \{/,/^\}/' "$RELEASE_SH")"
    case "$RESTAMP" in
        *RELEASE_STAGING_NOTARIZED*) pass "the manifest restamp carries the notarization state forward" ;;
        *) fail "restage_manifest_for_commit drops the notarized flag — a re-fold would launder a deferred staging" ;;
    esac
    # The DMG-verify leg asserts a stapled ticket, so arming it for a deferred DMG
    # would guarantee a red run.
    case "$(awk '/^arm_dmg_gate_if_notarized\(\) \{/,/^\}/' "$RELEASE_SH")" in
        *release_staging_is_notarized*) pass "the rc prerelease is armed only for a notarized staging" ;;
        *) fail "arm_dmg_gate_if_notarized does not consult the manifest" ;;
    esac
    if grep -q 'refresh_release_candidate_prerelease$' "$RELEASE_SH" \
       && [ "$(grep -c '^  refresh_release_candidate_prerelease$' "$RELEASE_SH")" -eq 0 ]; then
        pass "every Phase-A caller goes through the notarization-aware wrapper"
    else
        fail "a Phase-A path still calls refresh_release_candidate_prerelease directly"
    fi

    echo ""
    echo "test: --attach-notarized finishes a deferred release and refuses anything else"
    # Through `sandboxed` for the reason documented on its definition above: a
    # minimal PATH (so release_events.sh no-ops instead of emitting phantom
    # ReleaseStep* events into the developer's workspace) and no signing
    # credentials (so nothing can run on into the release-candidate force-push).
    out="$(sandboxed "$RELEASE_SH" --attach-notarized 99.99.99 2>&1)"; rc=$?
    if [ $rc -ne 0 ]; then pass "attaching without Phase A state is refused"; else fail "attach accepted a version with no state (rc=$rc)"; fi
    case "$out" in
        *"DEFERRED release"*) pass "explains that it finishes a deferred release" ;;
        *) fail "refusal does not explain what the phase is for: $out" ;;
    esac
    out="$(sandboxed "$RELEASE_SH" --attach-notarized -c /tmp/whatever 99.99.99 2>&1)"; rc=$?
    if [ $rc -ne 0 ]; then pass "--attach-notarized takes no changelog"; else fail "attach accepted -c (rc=$rc)"; fi
    out="$(sandboxed "$RELEASE_SH" --publish-verified --defer-notarization 99.99.99 2>&1)"; rc=$?
    if [ $rc -ne 0 ]; then pass "--defer-notarization is refused on --publish-verified"; else fail "publish accepted --defer-notarization (rc=$rc)"; fi
    case "$out" in
        *"read from the staging manifest"*) pass "says the publish reads the state from the manifest" ;;
        *) fail "refusal does not explain where deferral is decided: $out" ;;
    esac
    case "$(sandboxed "$RELEASE_SH" 2>&1)" in
        *"--attach-notarized"*) pass "usage documents --attach-notarized" ;;
        *) fail "usage does not mention --attach-notarized" ;;
    esac

    echo ""
    echo "test: the attach step orders its irreversible steps safely"
    ATTACH="$(awk '/^run_attach_notarized\(\) \{/,/^\}/' "$RELEASE_SH")"
    # The banner is the ONLY warning a user gets about the pending DMG. Removing it
    # before the stapled asset is actually up would leave a Gatekeeper block with no
    # explanation, so the upload must come first.
    UPLOAD_LINE="$(printf '%s\n' "$ATTACH" | grep -n 'release-attach' | cut -d: -f1 | head -1)"
    EDIT_LINE="$(printf '%s\n' "$ATTACH" | grep -n 'gh release edit' | cut -d: -f1 | head -1)"
    if [ -n "$UPLOAD_LINE" ] && [ -n "$EDIT_LINE" ] && [ "$UPLOAD_LINE" -lt "$EDIT_LINE" ]; then
        pass "the stapled asset is uploaded before the banner is removed"
    else
        fail "the banner is dropped before the upload (upload=$UPLOAD_LINE edit=$EDIT_LINE)"
    fi
    # Likewise the site must not be told to bump until the asset exists.
    EMIT_LINE="$(printf '%s\n' "$ATTACH" | grep -n 'emit_release_dmg_notarized' | cut -d: -f1 | head -1)"
    if [ -n "$EMIT_LINE" ] && [ "$UPLOAD_LINE" -lt "$EMIT_LINE" ]; then
        pass "ReleaseDmgNotarized is emitted only after the upload"
    else
        fail "the site bump is announced before the asset is uploaded (upload=$UPLOAD_LINE emit=$EMIT_LINE)"
    fi
    # Re-verify the manifest after the resume: without it, a resume that somehow
    # staged nothing would still swap the asset.
    # shellcheck disable=SC2016 # matching the literal source text, not expanding it
    case "$ATTACH" in
        *'release_staging_is_notarized "$STAGING_DIR"'*) pass "re-asserts the staging is notarized before swapping" ;;
        *) fail "attach does not re-check the manifest after the resume" ;;
    esac
    # A published, un-notarizable DMG needs a stated way out.
    case "$ATTACH" in
        *"delete-asset"*) pass "names the recovery for an Invalid/Rejected verdict" ;;
        *) fail "no recovery path for a rejected verdict on an already-published asset" ;;
    esac
    # Cleanup is what Phase B deferred; it belongs here and nowhere else.
    case "$ATTACH" in
        *"worktree remove"*) pass "the attach step performs the cleanup the publish deferred" ;;
        *) fail "the deferred worktree/staging are never cleaned up" ;;
    esac

    # THE RETRY GAP. Stapling rewrites the manifest to notarized=true and drops the
    # resume handle, so a failure in the upload right after it leaves a PUBLISHED
    # release whose DMG is still pending and whose stapled bytes are sitting in
    # staging. Re-running must resume from the upload — refusing there would strand
    # it with no recovery at all (the deferred publish already deleted the rc branch,
    # so --publish-verified's preflight refuses too).
    case "$ATTACH" in
        *already_stapled*) pass "an already-stapled staging resumes instead of refusing" ;;
        *) fail "a re-run after a failed upload has no path forward — the release would stay pending forever" ;;
    esac
    STAPLED_LINE="$(printf '%s\n' "$ATTACH" | grep -n 'already_stapled=1' | cut -d: -f1 | head -1)"
    POLL_LINE="$(printf '%s\n' "$ATTACH" | grep -n 'resume-notarize' | cut -d: -f1 | head -1)"
    if [ -n "$STAPLED_LINE" ] && [ -n "$POLL_LINE" ] && [ "$STAPLED_LINE" -lt "$POLL_LINE" ]; then
        pass "the already-stapled check precedes (and skips) the poll"
    else
        fail "the resume poll is not skipped for an already-stapled staging (stapled=$STAPLED_LINE poll=$POLL_LINE)"
    fi
    # The published-release check must come FIRST, or a normal Phase A that was
    # never published would be mistaken for a half-finished attach.
    VIEW_LINE="$(printf '%s\n' "$ATTACH" | grep -n 'gh release view' | cut -d: -f1 | head -1)"
    if [ -n "$VIEW_LINE" ] && [ "$VIEW_LINE" -lt "$STAPLED_LINE" ]; then
        pass "an unpublished release is rejected before the resume-from-upload branch"
    else
        fail "the release-exists check does not precede the already-stapled branch (view=$VIEW_LINE stapled=$STAPLED_LINE)"
    fi

    echo ""
    echo "test: the deferred paths do not weaken the guards that already existed"
    # Each of these was a real regression caught in review; they are asserted
    # here so the deferred mode cannot quietly re-open them.

    # --attach-notarized is a SECOND poller entry point. A detector that only
    # knew --resume-notarize would let two pollers for one version run blind —
    # the 2026-07-28 incident this function exists for.
    case "$(awk '/^warn_on_concurrent_pollers\(\) \{/,/^\}/' "$RELEASE_SH")" in
        *attach-notarized*) pass "the concurrent-poller detector covers both poller entry points" ;;
        *) fail "warn_on_concurrent_pollers cannot see an --attach-notarized poller" ;;
    esac

    # The attach step --clobbers an asset on a PUBLISHED release, so it must run
    # the same identity guard Phase B runs before going public.
    case "$ATTACH" in
        *release_staging_assert_commit*) pass "attach asserts the staging still belongs to the published tree" ;;
        *) fail "attach swaps a public asset without Phase B's identity guard" ;;
    esac

    # A MISSING staging must still hard-fail (as it did before this branch);
    # only an existing manifest that says notarized:false may skip the gate.
    case "$(awk '/^arm_dmg_gate_if_notarized\(\) \{/,/^\}/' "$RELEASE_SH")" in
        *'manifest.json'*) pass "a missing staging still reaches the loud refusal, not the skip" ;;
        *) fail "arm_dmg_gate_if_notarized turns a broken Phase A into a silent gate skip" ;;
    esac

    # The re-fold gate must not carry a PENDING staging into a run that never
    # asked to defer — that would publish a deferred release nobody chose, with
    # the handle gone and no way to finish it.
    case "$(awk '/^refold_can_reuse_staged_build\(\) \{/,/^\}/' "$RELEASE_SH")" in
        *release_staging_is_notarized*) pass "the re-fold gate refuses a pending staging unless deferring" ;;
        *) fail "the re-fold gate would reuse an unstapled DMG for a non-deferred release" ;;
    esac

    echo ""
    echo "test: one changelog extractor, shared by publish and attach"
    if [ "$(grep -c 'release_notes_extract_section' "$PROJECT_DIR/scripts/lib/release_notes.sh")" -ge 1 ] \
       && grep -q 'release_notes_extract_section' "$PROJECT_DIR/scripts/release-to-lucidos.sh" \
       && grep -q 'release_notes_extract_section' "$RELEASE_SH"; then
        pass "publish and attach both call the shared extractor"
    else
        fail "the changelog extractor is not shared — the attach step could rewrite notes that differ from the published ones"
    fi
    if grep -q 'matched = *1\|matched=1' "$PROJECT_DIR/scripts/release-to-lucidos.sh"; then
        fail "release-to-lucidos.sh still has its own copy of the extraction awk"
    else
        pass "the duplicate extraction awk is gone from release-to-lucidos.sh"
    fi
fi

echo ""
echo "build_dmg: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
