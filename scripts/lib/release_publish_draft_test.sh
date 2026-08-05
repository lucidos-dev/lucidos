#!/usr/bin/env bash
# Tests for the DRAFT-then-publish wiring in scripts/release.sh and
# scripts/release-to-lucidos.sh.
# Run: ./scripts/lib/release_publish_draft_test.sh
#
# scripts/lib/release_draft_test.sh covers the lib those two scripts call. This
# covers how they call it, which is where the ordering invariant actually lives:
#
#   wait for the platform tarballs -> publish the draft -> emit LucidosReleased
#
# Every step of that order is load-bearing. Publishing before the assets are
# attached is the outage this whole change removes. Emitting LucidosReleased
# before the publish is worse than the outage it replaces: that event starts the
# site chain, so lucidos.dev would repoint at a release nobody can see and every
# download link on the page would 404.
#
# Two tiers:
#   • the refusals are exercised for REAL, by running release.sh --publish-draft
#     against a version that has no state and no worktree. It gets no further
#     than a local `git remote get-url`, so the assertion is offline and touches
#     no network, no mirror and no release;
#   • the orderings are read out of the two scripts. They cannot be executed
#     here (each one force-pushes a public mirror), so source order is the
#     honest instrument, and comment lines are stripped first so prose ABOUT the
#     rule cannot satisfy it.
# shellcheck disable=SC2016 # file-wide, and a genuine false positive throughout:
# every needle here is LITERAL text to find in a release script, so the '$' in
# `"$PHASE"`, `"$REPO_ROOT"` and `"$tag"` must reach grep UNEXPANDED. Expanding
# any of them (they are unset here) would turn the needle into the empty string
# and the assertion into a vacuous pass, which is the one failure mode a drift
# test cannot afford. Same reasoning, same directive as front_door_gate_test.sh.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
RELEASE_SH="$PROJECT_DIR/scripts/release.sh"
PUBLISH_SH="$PROJECT_DIR/scripts/release-to-lucidos.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

for f in "$RELEASE_SH" "$PUBLISH_SH"; do
    [ -r "$f" ] || { echo "ERROR: cannot read $f" >&2; exit 1; }
done

# line_of <fixed-string> <file>: the first matching line number, comments
# excluded, or empty.
line_of() {
    grep -vE '^[[:space:]]*#' "$2" | grep -nF -m1 -e "$1" | cut -d: -f1
}

# func_body <name> <file>: one shell function's body, comments stripped.
func_body() {
    awk -v fn="^$1\\\\(\\\\) \\\\{" '$0 ~ fn { inblk = 1 } inblk { print NR ": " $0 } inblk && /^\}$/ { exit }' "$2" \
        | grep -vE '^[0-9]+: [[:space:]]*#'
}

# ordered <label> <body> <needle>...: assert the needles appear in this order.
ordered() {
    local label="$1" body="$2"
    shift 2
    local needle prev=0 line ok=1
    for needle in "$@"; do
        line="$(printf '%s\n' "$body" | grep -nF -m1 -e "$needle" | cut -d: -f1)"
        if [ -z "$line" ]; then
            fail "$label: never calls '$needle'"
            return
        fi
        if [ "$line" -lt "$prev" ]; then
            ok=0
            fail "$label: '$needle' comes too early (relative line $line, after $prev)"
        fi
        prev="$line"
    done
    [ "$ok" -eq 1 ] && pass "$label: $*"
}

# ── 1. the refusals, run for real ─────────────────────────────────────────────
echo "test: --publish-draft refuses a version with nothing to finish"
set +e
out="$("$RELEASE_SH" --publish-draft 0.77.0 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ] \
   && printf '%s' "$out" | grep -q 'Nothing to finish for v0.77.0' \
   && printf '%s' "$out" | grep -q -- '--publish-verified 0.77.0'; then
    pass "it refuses, names both missing inputs, and points at the command that cuts a release"
else
    fail "expected a refusal naming the missing state; rc=$rc out: $out"
fi
# Nothing public may be touched on the way to that refusal.
if printf '%s' "$out" | grep -qi 'push\|upload\|publishing'; then
    fail "the refusal path mentions pushing or uploading; it must reach no destructive step: $out"
else
    pass "the refusal reaches no destructive step"
fi

echo ""
echo "test: --publish-draft cannot be combined with another phase"
set +e
out="$("$RELEASE_SH" --publish-verified --publish-draft 0.77.0 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q 'Cannot combine'; then
    pass "combining phases is refused"
else
    fail "expected a phase-combination refusal; rc=$rc out: $out"
fi

echo ""
echo "test: --allow-missing-tarballs is accepted as a flag, not read as the version"
set +e
out="$("$RELEASE_SH" --publish-draft --allow-missing-tarballs 0.77.0 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q 'Nothing to finish for v0.77.0'; then
    pass "the flag parses and the version still resolves"
else
    fail "expected the same refusal with the flag given; rc=$rc out: $out"
fi

# ── 2. release.sh: the phase is wired in ──────────────────────────────────────
echo ""
echo "test: release.sh dispatches the publish-draft phase"
if grep -qE '^\s+--publish-draft\)' "$RELEASE_SH"; then
    pass "--publish-draft is parsed"
else
    fail "--publish-draft is not parsed"
fi
if grep -q 'if \[\[ "$PHASE" == "publish-draft" \]\]; then' "$RELEASE_SH" \
   && grep -q '^  run_publish_draft$' "$RELEASE_SH"; then
    pass "the phase block calls run_publish_draft"
else
    fail "no publish-draft phase block, or it does not call run_publish_draft"
fi
# It must exit before the generic flow, which creates a worktree and builds.
dispatch_line="$(line_of 'if [[ "$PHASE" == "publish-draft" ]]; then' "$RELEASE_SH")"
worktree_line="$(line_of 'git -C "$REPO_ROOT" worktree add' "$RELEASE_SH")"
if [ -n "$dispatch_line" ] && [ -n "$worktree_line" ] && [ "$dispatch_line" -lt "$worktree_line" ]; then
    pass "the phase is dispatched before any worktree is created"
else
    fail "the publish-draft dispatch does not precede the worktree creation (dispatch=$dispatch_line worktree=$worktree_line)"
fi

# ── 3. release.sh: the ordering inside run_publish_draft ──────────────────────
echo ""
echo "test: run_publish_draft waits, THEN publishes, THEN emits, THEN settles"
BODY="$(func_body run_publish_draft "$RELEASE_SH")"
if [ -z "$BODY" ]; then
    fail "run_publish_draft is not defined in release.sh"
else
    ordered "run_publish_draft" "$BODY" \
        'release_draft_wait_then_publish' \
        'emit_lucidos_released' \
        'settle_after_publish'
fi

echo ""
echo "test: the override is passed through, and a refusal is fatal"
# The wait-refuse-publish sequence itself lives in release_draft.sh and is
# driven behaviourally by release_draft_test.sh (fake gh, assert no
# --draft=false). What belongs HERE is that this caller hands the override on
# rather than deciding for itself, and treats a non-zero return as fatal.
if printf '%s\n' "$BODY" | grep -q 'ALLOW_MISSING_TARBALLS'; then
    pass "the override reaches the shared sequence"
else
    fail "run_publish_draft never passes ALLOW_MISSING_TARBALLS, so the flag would do nothing"
fi
if printf '%s\n' "$BODY" | grep -q 'was NOT published'; then
    pass "a refused publish is fatal and says the release did not go out"
else
    fail "run_publish_draft does not fail on a refused publish"
fi

# ── 4. the shared tail has one definition and two callers ─────────────────────
echo ""
echo "test: settle_after_publish is defined once and called by both entry points"
# A second copy is how one entry point would quietly stop settling the source
# side, which is the v0.17.0 failure: the release went out and main never
# learned its own version, so the site kept serving the previous DMG.
defs="$(grep -cE '^settle_after_publish\(\) \{' "$RELEASE_SH")"
calls="$(grep -cE '^\s+settle_after_publish "' "$RELEASE_SH")"
if [ "$defs" -eq 1 ] && [ "$calls" -eq 2 ]; then
    pass "one definition, two callers"
else
    fail "expected 1 definition and 2 callers of settle_after_publish; got $defs and $calls"
fi
for caller in run_publish_verified run_publish_draft; do
    if func_body "$caller" "$RELEASE_SH" | grep -q 'settle_after_publish "'; then
        pass "$caller settles the source side through the shared tail"
    else
        fail "$caller does not call settle_after_publish"
    fi
done
if func_body settle_after_publish "$RELEASE_SH" | grep -q 'settle_source_side'; then
    pass "the shared tail lands the bump on main"
else
    fail "the shared tail no longer calls settle_source_side"
fi

echo ""
echo "test: the override reaches release-to-lucidos.sh on the promotion path"
if func_body run_publish_verified "$RELEASE_SH" | grep -q 'promote_args+=(--allow-missing-tarballs)'; then
    pass "--allow-missing-tarballs is passed through to the publisher"
else
    fail "Phase B swallows --allow-missing-tarballs, so the flag would do nothing on the path that normally publishes"
fi

# ── 5. release-to-lucidos.sh: the draft, the order, the refusal ───────────────
echo ""
echo "test: release-to-lucidos.sh creates the GitHub Release as a DRAFT"
if grep -qF 'gh release create "$tag" --repo "$REPO_SLUG" --draft --target' "$PUBLISH_SH"; then
    pass "the create carries --draft"
else
    fail "the release is not created as a draft, so it goes public before its tarballs are attached"
fi
# The adopt branch must NOT pass a draft flag: `gh release edit` leaves the
# draft bit as it is, which is what keeps a re-run after a COMPLETED publish
# from un-publishing a live release.
edit_line="$(grep -nF 'gh release edit "$tag" --repo "$REPO_SLUG" --notes-file' "$PUBLISH_SH" | head -n 1)"
if [ -n "$edit_line" ]; then
    case "$edit_line" in
        *--draft*) fail "the adopt branch passes --draft to gh release edit, which would un-publish an already-published release" ;;
        *) pass "the adopt branch leaves the draft bit untouched" ;;
    esac
else
    fail "could not find the adopt branch's gh release edit"
fi

echo ""
echo "test: release-to-lucidos.sh waits, THEN publishes, THEN emits LucidosReleased"
PUB_CODE="$(grep -vE '^[[:space:]]*#' "$PUBLISH_SH")"
ordered "release-to-lucidos.sh" "$PUB_CODE" \
    'release_draft_wait_then_publish' \
    'emit_lucidos_released'

echo ""
echo "test: a refused publish is fatal there too, and names the resume"
if printf '%s\n' "$PUB_CODE" | grep -q 'was NOT published' \
   && printf '%s\n' "$PUB_CODE" | grep -q -- '--publish-draft'; then
    pass "the publisher dies on a refusal and names the command that finishes the job"
else
    fail "release-to-lucidos.sh does not treat a refused publish as fatal, or does not name --publish-draft"
fi
if printf '%s\n' "$PUB_CODE" | grep -qE '^\s+--allow-missing-tarballs\) ALLOW_MISSING_TARBALLS=1'; then
    pass "the override is parseable there"
else
    fail "release-to-lucidos.sh does not parse --allow-missing-tarballs"
fi

echo ""
echo "test: both scripts source the shared lib and its stem dependency"
for f in "$RELEASE_SH" "$PUBLISH_SH"; do
    name="$(basename "$f")"
    if grep -q 'lib/release_draft.sh' "$f" && grep -q 'lib/headless_tarball.sh' "$f"; then
        pass "$name sources release_draft.sh and headless_tarball.sh"
    else
        fail "$name does not source both release_draft.sh and headless_tarball.sh (the expected-asset list needs headless_tarball_stem)"
    fi
done

echo ""
echo "── the draft publish wiring: $PASS passed, $FAIL failed ──"
[ "$FAIL" -eq 0 ]
