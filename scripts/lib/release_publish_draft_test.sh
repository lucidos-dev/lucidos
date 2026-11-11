#!/usr/bin/env bash
# Tests for the DRAFT-then-publish wiring in scripts/release.sh and
# scripts/release-to-lucidos.sh.
# Run: ./scripts/lib/release_publish_draft_test.sh
#
# scripts/lib/release_draft_test.sh covers the lib those two scripts call. This
# covers how they call it, which is where the ordering invariant actually lives:
#
#   wait for the platform tarballs -> publish the draft -> move the mirror's
#   main (ADR 0642) -> emit LucidosReleased
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
#   • the orderings are read out of the two scripts. The whole scripts cannot
#     run here (each one pushes to a public mirror), so source order is the
#     instrument, and comment lines are stripped first so prose ABOUT the
#     rule cannot satisfy it. Section 6 runs the two publish tails' real text
#     with every effect stubbed, so the main-last order is executed too.
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
out="$(GH_TOKEN=offline-test-token "$RELEASE_SH" --publish-draft 0.77.0 2>&1)"
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
out="$(GH_TOKEN=offline-test-token "$RELEASE_SH" --publish-verified --publish-draft 0.77.0 2>&1)"
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
out="$(GH_TOKEN=offline-test-token "$RELEASE_SH" --publish-draft --allow-missing-tarballs 0.77.0 2>&1)"
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
echo "test: run_publish_draft waits, THEN publishes, THEN settles and announces"
BODY="$(func_body run_publish_draft "$RELEASE_SH")"
if [ -z "$BODY" ]; then
    fail "run_publish_draft is not defined in release.sh"
else
    ordered "run_publish_draft" "$BODY" \
        'release_draft_wait_then_publish' \
        'settle_after_publish'
    # The v0.39.3 bug: this function emitted LucidosReleased itself, BEFORE
    # settle_after_publish landed the bump, so the site built install.sh from a
    # main that still named the previous version.
    if printf '%s\n' "$BODY" | grep -q 'emit_lucidos_released'; then
        fail "run_publish_draft emits LucidosReleased itself, ahead of the landing"
    else
        pass "run_publish_draft leaves the announce to settle_after_publish"
    fi
fi

echo ""
echo "test: settle_after_publish announces only through settle_and_announce_release"
SETTLE_BODY="$(func_body settle_after_publish "$RELEASE_SH" || true)"
if printf '%s\n' "$SETTLE_BODY" | grep -q 'settle_and_announce_release "$published_commit" "$dmg_pending"'; then
    pass "settle_after_publish settles and announces in one call"
else
    fail "settle_after_publish does not call settle_and_announce_release with the published commit"
fi
ANNOUNCE_BODY="$(func_body settle_and_announce_release "$RELEASE_SH" || true)"
ordered "settle_and_announce_release" "$ANNOUNCE_BODY" \
    'settle_source_side' \
    'emit_lucidos_released' \
    'fail "$LANDING_CONFLICT"'

echo ""
echo "test: release.sh tells release-to-lucidos.sh not to announce, on both invocations"
# Without the flag, release-to-lucidos.sh emits at its end, which is before
# release.sh lands the bump: the v0.39.3 order.
if func_body run_publish_verified "$RELEASE_SH" | grep -q -- '--no-released-event'; then
    pass "Phase B passes --no-released-event"
else
    fail "Phase B lets release-to-lucidos.sh announce before the landing"
fi
ONESHOT_CALLS="$(grep -vE '^[[:space:]]*#' "$RELEASE_SH" | grep -cF '"$WORKTREE_DIR/scripts/release-to-lucidos.sh" --no-released-event' || true)"
if [ "$ONESHOT_CALLS" = "2" ]; then
    pass "both one-shot invocations pass --no-released-event"
else
    fail "one-shot invocations carrying --no-released-event: $ONESHOT_CALLS (want 2)"
fi
oneshot_publish="$(line_of '"$WORKTREE_DIR/scripts/release-to-lucidos.sh" --no-released-event' "$RELEASE_SH" || true)"
oneshot_announce="$(grep -vE '^[[:space:]]*#' "$RELEASE_SH" | grep -nE '^settle_and_announce_release ' | head -1 | cut -d: -f1 || true)"
if [ -n "$oneshot_publish" ] && [ -n "$oneshot_announce" ] && [ "$oneshot_publish" -lt "$oneshot_announce" ]; then
    pass "the one-shot announces after release-to-lucidos.sh returns"
else
    fail "the one-shot tail does not announce after publishing (publish=$oneshot_publish announce=$oneshot_announce)"
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
if func_body settle_after_publish "$RELEASE_SH" | grep -q 'settle_and_announce_release'; then
    pass "the shared tail lands the bump on main (and then announces)"
else
    fail "the shared tail no longer calls settle_and_announce_release"
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
echo "test: release-to-lucidos.sh emits only when its caller did not take the announce"
if printf '%s\n' "$PUB_CODE" | grep -qE '^\s+--no-released-event\) EMIT_RELEASED=0'; then
    pass "--no-released-event is parsed"
else
    fail "release-to-lucidos.sh does not parse --no-released-event"
fi
# Every emit must sit inside the EMIT_RELEASED guard.
if printf '%s\n' "$PUB_CODE" | awk '
    /^if \[\[ "\$EMIT_RELEASED" == "1" \]\]; then/ { guard = 1; next }
    guard && /^else$|^fi$/ { guard = 0 }
    /emit_lucidos_released/ && !guard { bad = 1 }
    END { exit bad }'; then
    pass "every LucidosReleased emit is behind the EMIT_RELEASED guard"
else
    fail "release-to-lucidos.sh emits LucidosReleased outside the EMIT_RELEASED guard"
fi

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

# ── 6. the mirror's main moves last, executed with stubbed effects ────────────
# The real source text runs here, with every effect replaced by a recorder.
# What it proves: main moves only after a successful publish, never after a
# failed wait, and LucidosReleased (settle_after_publish) only after main moved.
STUB_LOG="$(mktemp)"
STUB_STATE="$(mktemp)"
trap 'rm -f "$STUB_LOG" "$STUB_STATE"' EXIT
printf 'RELEASE_VERSION=0.77.0\n' >"$STUB_STATE"

# stubbed_run <wait-rc> <advance-rc> <code> [draft-state]: run <code> under the
# recorders and print the log, one call per line.
stubbed_run() {
    : >"$STUB_LOG"
    WAIT_RC="$1" ADV_RC="$2" CODE="$3" DRAFT_STATE="${4:-draft}" LOG="$STUB_LOG" STATE_FILE="$STUB_STATE" bash -c '
        set -Eeuo pipefail
        fail() { echo "fail" >>"$LOG"; exit 1; }
        step() { :; }; begin_step() { :; }; end_step() { :; }
        gh() { :; }
        git() {
            case "$*" in
                *"show -s"*) echo PARENT ;;
                *) return 0 ;;
            esac
        }
        release_mirror_branch() { printf main; }
        release_mirror_fetch() { :; }
        release_mirror_tag_sha() { echo COMMIT; }
        release_draft_state() { echo "$DRAFT_STATE"; }
        release_draft_wait_then_publish() { echo "publish $4" >>"$LOG"; return "$WAIT_RC"; }
        release_mirror_main_advance() { echo "advance $3 $4" >>"$LOG"; return "$ADV_RC"; }
        settle_after_publish() { echo "settle" >>"$LOG"; }
        REPO_ROOT=/nonexistent REMOTE=lucidos REPO_SLUG=lucidos-dev/lucidos
        VERSION=0.77.0 NEW_TAG=v0.77.0 WORKTREE_DIR=/nonexistent STAGING_DIR=/nonexistent
        ALLOW_MISSING_TARBALLS=0
        release=0.77.0 tag=v0.77.0 commit=COMMIT parent=PARENT BRANCH=main
        eval "$CODE"
        if declare -F run_publish_draft >/dev/null; then run_publish_draft; fi
    ' >/dev/null 2>&1 || true
    tr '\n' ',' <"$STUB_LOG"
}

echo ""
echo "test: release-to-lucidos.sh moves main only after the publish succeeds (executed)"
PUB_TAIL="$(awk '/^begin_step event "Waiting/{on=1} on{print} on && /did not move to it/{exit}' "$PUBLISH_SH")"
if [ -z "$PUB_TAIL" ]; then
    fail "could not extract the publish tail of release-to-lucidos.sh"
else
    got="$(stubbed_run 0 0 "$PUB_TAIL")"
    if [ "$got" = "publish COMMIT,advance COMMIT PARENT," ]; then
        pass "publish, then main advances from the parent to the commit"
    else
        fail "expected publish then advance, got: $got"
    fi
    got="$(stubbed_run 1 0 "$PUB_TAIL")"
    if [ "$got" = "publish COMMIT,fail," ]; then
        pass "a failed wait fails the run and never moves main"
    else
        fail "a failed wait must not move main, got: $got"
    fi
fi

echo ""
echo "test: a rival in-flight tag after the tag push backs this release off (executed)"
# race_run <tag-before> <check-rc>: run the tag push and its recount under
# recorders, and print the calls, one per comma.
RACE_CODE="$(awk '/^tag_was_present=/{on=1} on{print} on && /^fi$/{exit}' "$PUBLISH_SH")"
race_run() {
    : >"$STUB_LOG"
    TAG_BEFORE="$1" CHECK_RC="$2" CODE="$RACE_CODE" LOG="$STUB_LOG" bash -c '
        set -Eeuo pipefail
        fail() { echo "fail" >>"$LOG"; exit 1; }
        release_mirror_tag_sha() { printf "%s" "$TAG_BEFORE"; }
        release_tree_push() { echo "push $3 $4" >>"$LOG"; }
        release_mirror_history_check() { echo "check $3 $4" >>"$LOG"; return "$CHECK_RC"; }
        release_tree_delete_release_tag() { echo "delete $3" >>"$LOG"; }
        REPO_ROOT=/nonexistent REMOTE=lucidos BRANCH=main
        release=0.77.0 tag=v0.77.0 commit=COMMIT mirror_main=PARENT
        eval "$CODE"
    ' >/dev/null 2>&1 || true
    tr '\n' ',' <"$STUB_LOG"
}
if [ -z "$RACE_CODE" ]; then
    fail "could not extract the tag push and its recount from release-to-lucidos.sh"
else
    got="$(race_run "" 0)"
    if [ "$got" = "push COMMIT refs/tags/v0.77.0,check PARENT v0.77.0," ]; then
        pass "the tag push is recounted against main, and a clean count goes on"
    else
        fail "expected push then check, got: $got"
    fi
    got="$(race_run "" 1)"
    if [ "$got" = "push COMMIT refs/tags/v0.77.0,check PARENT v0.77.0,delete v0.77.0,fail," ]; then
        pass "a rival tag deletes the tag this run created, then refuses"
    else
        fail "a rival tag must delete this run's new tag and refuse, got: $got"
    fi
    got="$(race_run COMMIT 1)"
    if [ "$got" = "push COMMIT refs/tags/v0.77.0,check PARENT v0.77.0,fail," ]; then
        pass "a tag an earlier run pushed is kept, and the run refuses"
    else
        fail "a resumed run must keep its earlier tag, got: $got"
    fi
fi

echo ""
echo "test: run_publish_draft publishes, moves main, THEN settles (executed)"
DRAFT_FN="$(awk '/^run_publish_draft\(\) \{/{on=1} on{print} on && /^\}$/{exit}' "$RELEASE_SH")"
if [ -z "$DRAFT_FN" ]; then
    fail "could not extract run_publish_draft from release.sh"
else
    got="$(stubbed_run 0 0 "$DRAFT_FN" draft)"
    if [ "$got" = "publish COMMIT,advance COMMIT PARENT,settle," ]; then
        pass "a draft is published, main moves, and only then the release settles"
    else
        fail "expected publish, advance, settle; got: $got"
    fi
    got="$(stubbed_run 0 0 "$DRAFT_FN" published)"
    if [ "$got" = "advance COMMIT PARENT,settle," ]; then
        pass "an already-published release still moves main before it settles"
    else
        fail "a published release with main behind must move main; got: $got"
    fi
    got="$(stubbed_run 1 0 "$DRAFT_FN" draft)"
    if [ "$got" = "publish COMMIT,fail," ]; then
        pass "a failed wait moves no main and settles nothing"
    else
        fail "a failed wait must stop before main; got: $got"
    fi
    got="$(stubbed_run 0 1 "$DRAFT_FN" draft)"
    if [ "$got" = "publish COMMIT,advance COMMIT PARENT,fail," ]; then
        pass "a refused main advance stops before LucidosReleased"
    else
        fail "a refused advance must not settle; got: $got"
    fi
fi

echo ""
echo "── the draft publish wiring: $PASS passed, $FAIL failed ──"
[ "$FAIL" -eq 0 ]
