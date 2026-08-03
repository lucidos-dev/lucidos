#!/usr/bin/env bash
# Tests for release.sh's release-candidate GATE ARTIFACT handling: the rc-<version>
# DRAFT release, the dispatch that fires dmg-verify at it, and the cleanup that
# removes it (ADR 0036).
#
# What this pins, and why each one has already been a bug or is one waiting:
#
#   1. the rc release is created with BOTH --draft and --prerelease. Without
#      --draft it is publicly listed, which is the whole defect: rc-0.19.1 sat
#      above the GA on github.com/lucidos-dev/lucidos/releases until it was
#      deleted by hand on 2026-08-03.
#   2. the gate is DISPATCHED, after the release exists. A draft emits no
#      webhook at all (GitHub does not trigger workflows for the created /
#      edited / deleted activity types on drafts), so nothing else fires
#      dmg-verify, and dispatching before the upload would race the download.
#   3. a dispatch that cannot be queued is FATAL and names --push-rc. A silently
#      unarmed gate looks exactly like a passing one, right before an
#      irreversible publish.
#   4. no code path passes `gh release delete --cleanup-tag` any more. That flag
#      deletes the release FIRST and then DELETEs refs/tags/<tag>, so a draft's
#      absent ref becomes a non-zero exit AFTER the release is already gone:
#      once fatal to a step that fully succeeded, once a warning about something
#      that was in fact deleted.
#   5. cleanup still removes the tag ref of a LEGACY non-draft rc, which does
#      have one, and reports success for a draft, which does not.
#   6. cleanup tolerates an rc release that does not exist at all. That is the
#      live v0.19.1 state (its rc release was removed by hand), and Phase B must
#      not abort on it.
#
# The subject functions live in release.sh, a script rather than a library, so
# they are extracted with awk, the same technique release_refold_gate_test.sh
# and build_dmg_test.sh use. `gh` and `git` are shell-function stubs modelling
# just enough server state to answer "does the release exist" and "does the tag
# ref exist"; a stub that always succeeded could not express findings 4 to 6.
# Each call runs in a subshell under release.sh's own `set -Eeuo pipefail`, both
# because `fail` exits and because the option set is part of the behaviour.
#
# Hermetic and offline: no network, no gh, no git, no release.
# Run: ./scripts/lib/release_rc_gate_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
RELEASE_SH="$PROJECT_DIR/scripts/release.sh"
WORKFLOW="$PROJECT_DIR/.github/workflows/install-smoke.yml"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail_t() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

[ -r "$WORKFLOW" ] || { echo "ERROR: cannot read $WORKFLOW" >&2; exit 1; }

summary() {
  echo
  echo "release rc gate: $PASS passed, $FAIL failed"
  [ "$FAIL" -eq 0 ]
}

# ── the CI side of the gate ───────────────────────────────────────────────────
# Defined up here rather than written inline at the end, because it is the half
# that still runs when release.sh is absent (see the skip below).
#
# Drift assertions on install-smoke.yml, comment lines stripped so the file's own
# prose about a rule can neither satisfy nor violate it.
workflow_assertions() {
  echo
  echo "test: install-smoke.yml can still read the draft and fire on the dispatch"

  job_code() {
    awk -v key="  $1:" '
      $0 == key   { inblk = 1; print; next }
      inblk && /^  [A-Za-z0-9_-]+:[[:space:]]*$/ { exit }
      inblk       { print }
    ' "$WORKFLOW" | grep -vE '^[[:space:]]*#'
  }

  local dmg_job types job_count guard_count
  dmg_job="$(job_code dmg-verify)"
  if [ -z "$dmg_job" ]; then
    fail_t "the dmg-verify job is gone from $WORKFLOW"
  else
    if printf '%s\n' "$dmg_job" | grep -A2 '^    permissions:' | grep -q 'contents: write'; then
      pass "dmg-verify declares contents: write (what makes a DRAFT readable)"
    else
      fail_t "dmg-verify does not declare contents: write, so it cannot see the rc draft"
    fi
    if printf '%s\n' "$dmg_job" | grep -q "workflow_dispatch' && inputs.dmg_tag != ''"; then
      pass "dmg-verify still fires on a dispatch carrying dmg_tag"
    else
      fail_t "dmg-verify no longer accepts the dmg_tag dispatch, which is the only rc trigger"
    fi
    if printf '%s\n' "$dmg_job" | grep -q 'github.event.release.prerelease == true'; then
      pass "the legacy non-draft rc prerelease arm is kept"
    else
      fail_t "the release-event arm was dropped; a hand-made rc would no longer gate"
    fi
  fi

  # `created` never fires for a draft, and WOULD double-run dmg-verify for a
  # non-draft rc, which already matches prereleased.
  types="$(grep -A1 '^  release:' "$WORKFLOW" | grep 'types:')"
  case "$types" in
    *created*) fail_t "'created' is back in the release types: $types" ;;
    *)         pass "'created' is absent from the release types" ;;
  esac

  # A gate dispatch must start dmg-verify and nothing else: every OTHER job guards
  # on the input being empty. The job keys are counted from inside the `jobs:`
  # block, since the trigger keys under `on:` share their indentation.
  job_count="$(awk '
    /^jobs:/                                   { inj = 1; next }
    inj && /^[A-Za-z]/                         { exit }
    inj && /^  [A-Za-z0-9_-]+:[[:space:]]*$/   { n++ }
    END                                        { print n + 0 }
  ' "$WORKFLOW")"
  guard_count="$(grep -cF "inputs.dmg_tag == ''" "$WORKFLOW")"
  if [ "$guard_count" -eq $((job_count - 1)) ]; then
    pass "all $guard_count non-dmg-verify jobs still guard on an empty dmg_tag"
  else
    fail_t "expected $((job_count - 1)) dmg_tag guards for $job_count jobs, found $guard_count"
  fi
}

# This file SHIPS to the public mirror while its main subject, scripts/release.sh,
# is stripped from it (RELEASE_TREE_EXCLUDE_PATHS). So the release.sh half skips
# rather than failing, exactly as build_dmg_test.sh does for the same reason: a
# contributor running the suite from a clone of the mirror must not get a run of
# failures for a file that was never published. The install-smoke.yml half below
# still runs there, because that workflow does ship.
if [ ! -r "$RELEASE_SH" ]; then
  echo "  skip: scripts/release.sh is not present (stripped from the public mirror),"
  echo "        so the gate-function assertions cannot run. The workflow half still can."
  workflow_assertions
  summary
  exit
fi

# ── extraction ────────────────────────────────────────────────────────────────
EXTRACT="$(mktemp)"
for fn in rc_release_delete refresh_release_candidate_draft \
          delete_release_candidate dispatch_dmg_verify; do
  awk -v pat="^$fn\\\\(\\\\) \\\\{" '$0 ~ pat, /^\}/' "$RELEASE_SH" >> "$EXTRACT"
  printf '\n' >> "$EXTRACT"
done
for fn in rc_release_delete refresh_release_candidate_draft \
          delete_release_candidate dispatch_dmg_verify; do
  grep -q "^$fn()" "$EXTRACT" \
    || { echo "ERROR: could not extract $fn from release.sh" >&2; exit 1; }
done

# `step` and `fail` are one-liners / emit helpers in release.sh, so they are
# reproduced here rather than extracted. Section 8 asserts the real `fail` still
# has the contract this stub models (a message, then exit 1), so the two cannot
# drift apart silently.
step() { echo "==> $*"; }
fail() { echo "ERROR: $*" >&2; exit 1; }

# shellcheck source=/dev/null
source "$EXTRACT"
rm -f "$EXTRACT"

# ── the fixture ───────────────────────────────────────────────────────────────
# Four of these look unused to ShellCheck and are not: their consumers are the
# functions extracted from release.sh, which arrive through a temp file that is
# sourced and deleted at runtime, so the cross-file reference is invisible. Each
# is disabled at its own line rather than file-wide, so a genuinely unused
# variable added later still reports.
VERSION="0.77.0"
RC_TAG="rc-$VERSION"
RC_BRANCH="rc/$VERSION"
REPO_SLUG="lucidos-dev/lucidos"
# shellcheck disable=SC2034 # read by delete_release_candidate
REMOTE="lucidos"
# shellcheck disable=SC2034 # read by delete_release_candidate
REPO_ROOT="/nonexistent-repo"
# shellcheck disable=SC2034 # read by delete_release_candidate
RC_LOCAL_REF="refs/release-candidates/$VERSION"
STAGING_DIR="$(mktemp -d)"
GH_LOG="$(mktemp)"
trap 'rm -rf "$STAGING_DIR" "$GH_LOG"' EXIT
printf 'dmg bytes\n' > "$STAGING_DIR/Lucidos_${VERSION}_aarch64.dmg"
printf 'sig\n'       > "$STAGING_DIR/Lucidos.app.tar.gz.sig"

# Server state the stubs model. Reset per case by new_case.
RC_EXISTS=0        # a release named $RC_TAG is present (draft or not)
RC_TAG_REF=0       # refs/tags/$RC_TAG resolves (a draft never has one)
GH_CREATE_OK=1
GH_DELETE_OK=1
GH_DISPATCH_OK=1
RC_BRANCH_ON_REMOTE=1

# gh stub: records every invocation, then answers from the state above.
gh() {
  printf '%s\n' "$*" >> "$GH_LOG"
  case "${1:-}" in
    release)
      case "${2:-}" in
        view)   [ "$RC_EXISTS" = 1 ] ;;
        delete) [ "$GH_DELETE_OK" = 1 ] && { RC_EXISTS=0; return 0; }; return 1 ;;
        create) [ "$GH_CREATE_OK" = 1 ] && { RC_EXISTS=1; return 0; }; return 1 ;;
        *)      return 0 ;;
      esac
      ;;
    # The only gh api call on these paths is the tag-ref delete, which 404s when
    # no ref exists. That 404 is exactly what --cleanup-tag turned into a failed
    # command, so the stub must reproduce it rather than always succeeding.
    api)      [ "$RC_TAG_REF" = 1 ] && { RC_TAG_REF=0; return 0; }; return 1 ;;
    workflow) [ "$GH_DISPATCH_OK" = 1 ] ;;
    *)        return 0 ;;
  esac
}
git() { printf 'git %s\n' "$*" >> "$GH_LOG"; return 0; }
release_rc_remote_sha() { [ "$RC_BRANCH_ON_REMOTE" = 1 ] && echo "deadbeef"; return 0; }
release_staging_is_notarized() { return 0; }
emit_release_step() { :; }
# shellcheck disable=SC2034 # read by the extracted release.sh `fail` contract
RELEASE_STEP=""

new_case() {
  : > "$GH_LOG"
  RC_EXISTS="${1:-0}"
  RC_TAG_REF="${2:-0}"
  GH_CREATE_OK=1
  GH_DELETE_OK=1
  GH_DISPATCH_OK=1
  RC_BRANCH_ON_REMOTE=1
}

# Run one subject function the way release.sh runs it, and report its status.
# The subshell is required: `fail` exits, and these are exit-path tests.
run_fn() {
  local out
  out="$( set -Eeuo pipefail; "$@" 2>&1 )"
  RUN_STATUS=$?
  RUN_OUT="$out"
}

logged()     { grep -qF -- "$1" "$GH_LOG"; }
log_line_of() { grep -nF -- "$1" "$GH_LOG" | head -1 | cut -d: -f1; }

# Assertion helpers. They exist so no assertion is written as `cond && pass ||
# fail_t`, which is not if-then-else: a `pass` that ever returned non-zero would
# run the failure branch too.
assert_ok()   { if [ "$RUN_STATUS" = 0 ]; then pass "$1"; else fail_t "$2 (exit $RUN_STATUS): $RUN_OUT"; fi; }
assert_fail() { if [ "$RUN_STATUS" != 0 ]; then pass "$1"; else fail_t "$2: $RUN_OUT"; fi; }
assert_logged()     { if logged "$1"; then pass "$2"; else fail_t "$3: $(cat "$GH_LOG")"; fi; }
assert_not_logged() { if logged "$1"; then fail_t "$3: $(cat "$GH_LOG")"; else pass "$2"; fi; }

# ── 1. the rc release is a DRAFT, and still flagged prerelease ────────────────
echo
echo "test: the rc release is created as a draft prerelease at the rc branch"
new_case 0 0
run_fn refresh_release_candidate_draft
[ "$RUN_STATUS" = 0 ] || fail_t "arming a fresh rc exited $RUN_STATUS: $RUN_OUT"
create="$(grep -F 'release create' "$GH_LOG" | head -1)"
case "$create" in
  *" --draft "*) pass "gh release create passes --draft (never publicly listed)" ;;
  *)             fail_t "gh release create has no --draft: $create" ;;
esac
case "$create" in
  *" --prerelease "*) pass "gh release create still passes --prerelease" ;;
  *)                  fail_t "gh release create dropped --prerelease: $create" ;;
esac
case "$create" in
  *" --target $RC_BRANCH "*) pass "the draft targets $RC_BRANCH" ;;
  *)                         fail_t "the draft does not target $RC_BRANCH: $create" ;;
esac
case "$create" in
  *"$RC_TAG"*) pass "the draft is named $RC_TAG" ;;
  *)           fail_t "the draft is not named $RC_TAG: $create" ;;
esac

# ── 2. the gate is dispatched, AFTER the release exists ───────────────────────
echo
echo "test: arming dispatches dmg-verify at the draft, after creating it"
if logged "workflow run install-smoke.yml"; then
  pass "install-smoke.yml is dispatched (a draft fires no release event)"
else
  fail_t "no workflow dispatch recorded: $(cat "$GH_LOG")"
fi
if logged "dmg_tag=$RC_TAG"; then
  pass "the dispatch carries dmg_tag=$RC_TAG"
else
  fail_t "the dispatch does not carry dmg_tag=$RC_TAG: $(cat "$GH_LOG")"
fi
if logged "--ref $RC_BRANCH"; then
  pass "the dispatch pins --ref $RC_BRANCH (the candidate's own workflow, not the mirror default branch's)"
else
  fail_t "the dispatch does not pin --ref $RC_BRANCH: $(cat "$GH_LOG")"
fi
create_at="$(log_line_of 'release create')"
dispatch_at="$(log_line_of 'workflow run')"
if [ -n "$create_at" ] && [ -n "$dispatch_at" ] && [ "$create_at" -lt "$dispatch_at" ]; then
  pass "the release is created BEFORE the gate is dispatched (no download race)"
else
  fail_t "create/dispatch out of order (create=$create_at dispatch=$dispatch_at)"
fi

# ── 3. --cleanup-tag is gone, in the calls and in the source ──────────────────
echo
echo "test: no path passes --cleanup-tag (a draft has no tag ref to clean up)"
if logged "--cleanup-tag"; then
  fail_t "a gh call still passes --cleanup-tag: $(cat "$GH_LOG")"
else
  pass "no --cleanup-tag in the arming path's gh calls"
fi
# Comment lines are excluded: rc_release_delete's own comment explains why the
# flag is not used, and that explanation must not read as a use of it.
if grep -vE '^[[:space:]]*#' "$RELEASE_SH" | grep -q -- '--cleanup-tag'; then
  fail_t "release.sh code still passes --cleanup-tag"
else
  pass "no --cleanup-tag left in release.sh's code"
fi

# ── 4. a dispatch that cannot be queued is FATAL ──────────────────────────────
echo
echo "test: a gate dispatch that cannot be queued fails the arming step"
new_case 0 0
GH_DISPATCH_OK=0
run_fn refresh_release_candidate_draft
if [ "$RUN_STATUS" != 0 ]; then
  pass "a failed dispatch exits non-zero rather than reporting an armed gate"
else
  fail_t "a failed dispatch exited 0: $RUN_OUT"
fi
case "$RUN_OUT" in
  *"--push-rc"*) pass "the failure names the --push-rc retry" ;;
  *)             fail_t "the failure does not name --push-rc: $RUN_OUT" ;;
esac
case "$RUN_OUT" in
  *"NOTHING is verifying it"*) pass "the failure says the draft exists but is ungated" ;;
  *)                           fail_t "the failure does not say the gate is unarmed: $RUN_OUT" ;;
esac

# ── 5. a stale DRAFT is replaced, not tripped over ────────────────────────────
echo
echo "test: a stale rc DRAFT (release present, no tag ref) is replaced cleanly"
new_case 1 0
run_fn refresh_release_candidate_draft
assert_ok "refreshing over a stale draft exits 0 (an absent tag ref is not an error)" \
          "refreshing over a stale draft failed"
assert_logged "release delete $RC_TAG" \
  "the stale draft is deleted first" "the stale draft was not deleted"
assert_logged "release create" \
  "the replacement draft is created" "no replacement draft was created"

# ── 6. a stale release that will NOT delete is fatal, and nothing is created ──
echo
echo "test: a stale rc release that survives deletion aborts before creating"
new_case 1 0
GH_DELETE_OK=0
run_fn refresh_release_candidate_draft
assert_fail "an undeletable stale release aborts the step" \
            "an undeletable stale release exited 0"
assert_not_logged "release create" \
  "no replacement is created while the stale release survives" \
  "a replacement was created over a surviving stale release"

# ── 7. cleanup of a DRAFT: succeeds, and says nothing alarming ────────────────
echo
echo "test: delete_release_candidate removes a draft without a false warning"
new_case 1 0
run_fn delete_release_candidate
assert_ok "cleanup of a draft exits 0" "cleanup of a draft failed"
assert_logged "release delete $RC_TAG" \
  "the draft release is deleted" "the draft release was not deleted"
case "$RUN_OUT" in
  *"could not delete the $RC_TAG release"*)
    fail_t "cleanup warned about a release it did delete: $RUN_OUT" ;;
  *) pass "no 'could not delete' warning about a release that was deleted" ;;
esac

# ── 8. cleanup of a LEGACY non-draft rc: the tag ref goes too ─────────────────
echo
echo "test: delete_release_candidate still removes a legacy rc's tag ref"
new_case 1 1
run_fn delete_release_candidate
assert_ok "cleanup of a non-draft rc exits 0" "cleanup of a non-draft rc failed"
assert_logged "api -X DELETE repos/$REPO_SLUG/git/refs/tags/$RC_TAG" \
  "the tag ref is deleted for a legacy rc that has one" "the tag ref was not deleted"

# ── 9. cleanup with NO rc release at all (the live v0.19.1 state) ─────────────
echo
echo "test: delete_release_candidate skips cleanly when no rc release exists"
new_case 0 0
run_fn delete_release_candidate
assert_ok "cleanup exits 0 when the rc release is already gone" \
          "cleanup failed with no rc release present"
assert_not_logged "release delete" \
  "no delete is attempted for a release that does not exist" \
  "cleanup tried to delete a release that does not exist"

# ── 10. dispatch_dmg_verify reports, and the callers choose the severity ──────
echo
echo "test: dispatch_dmg_verify returns status, leaving severity to the caller"
new_case 0 0
run_fn dispatch_dmg_verify "v$VERSION" "v$VERSION"
assert_ok "a queued dispatch returns 0" "a queued dispatch reported failure"
new_case 0 0
GH_DISPATCH_OK=0
run_fn dispatch_dmg_verify "v$VERSION" "v$VERSION"
assert_fail "a dispatch that cannot be queued returns non-zero" \
            "an unqueued dispatch returned 0"

# The ref is what makes the candidate verify itself with the workflow it ships.
# Unpinned, `gh workflow run` takes the mirror's default branch, which is the
# PREVIOUS release's tree: that is how v0.20.0's gate ran v0.19.0's workflow,
# whose dmg-verify job had no `contents: write` and so could not read the rc
# draft at all. Refusing an empty ref is cheaper than diagnosing that twice.
new_case 0 0
run_fn dispatch_dmg_verify "v$VERSION"
assert_fail "a dispatch with no ref is refused" \
            "an unpinned dispatch was allowed (it would run the default branch's workflow)"

# The two callers must differ, and only the source can show that: Phase A pipes
# it into `fail` (the gate is all that stands before an irreversible publish),
# while --attach-notarized only warns (the asset is already published).
# shellcheck disable=SC2016 # the needles are LITERAL release.sh text, so the
# `$RC_TAG` / `$NEW_TAG` in them must reach grep unexpanded. Expanding either
# (both are set here, to different values) would make the assertion vacuous.
if grep -A2 'dispatch_dmg_verify "\$RC_TAG"' "$RELEASE_SH" | grep -q 'fail "'; then
  pass "Phase A treats a failed dispatch as fatal"
else
  fail_t "Phase A no longer fails on a dispatch it could not queue"
fi
# shellcheck disable=SC2016 # literal needle, as above
if grep -A3 'dispatch_dmg_verify "\$NEW_TAG"' "$RELEASE_SH" | grep -q 'WARNING'; then
  pass "--attach-notarized treats a failed dispatch as a warning"
else
  fail_t "--attach-notarized no longer warns on a dispatch it could not queue"
fi

# refresh_release_candidate_draft calls dispatch_dmg_verify, which is defined
# ~650 lines further down. Bash resolves that at call time, so it only works
# while every top-level phase invocation sits BELOW the definition. Moving a
# phase block up would break the gate with "command not found" at the worst
# possible moment, and nothing else in the file would notice.
def_at="$(grep -n '^dispatch_dmg_verify() {' "$RELEASE_SH" | cut -d: -f1)"
first_phase_at="$(grep -n '^  run_[a-z_]*$' "$RELEASE_SH" | head -1 | cut -d: -f1)"
if [ -n "$def_at" ] && [ -n "$first_phase_at" ] && [ "$def_at" -lt "$first_phase_at" ]; then
  pass "dispatch_dmg_verify is defined before the first phase runs (line $def_at < $first_phase_at)"
else
  fail_t "dispatch_dmg_verify (line ${def_at:-?}) is not defined before the first phase invocation (line ${first_phase_at:-?})"
fi

# The stub above models `fail` as "message, then exit 1". Assert the real one
# still does that, or every exit-path assertion here is testing a fiction.
if awk '/^fail\(\) \{/,/^\}/' "$RELEASE_SH" | grep -q '^  exit 1$'; then
  pass "release.sh's fail still exits 1 (the stub models it faithfully)"
else
  fail_t "release.sh's fail no longer ends in exit 1; this file's stub is stale"
fi

# ── 11. the CI side of the same gate ──────────────────────────────────────────
workflow_assertions

summary
