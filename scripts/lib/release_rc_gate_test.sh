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
# dispatch_dmg_verify classifies a failed dispatch and probes the ref through
# release_draft.sh, so the subject needs that lib. It is withheld from the
# public mirror alongside release.sh, which is why the skip below tests both.
DRAFT_LIB="$PROJECT_DIR/scripts/lib/release_draft.sh"
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
if [ ! -r "$RELEASE_SH" ] || [ ! -r "$DRAFT_LIB" ]; then
  echo "  skip: scripts/release.sh is not present (stripped from the public mirror),"
  echo "        so the gate-function assertions cannot run. The workflow half still can."
  workflow_assertions
  summary
  exit
fi

# The real classifier and the real ref probe, not stubs. Both are the subject:
# a copy here would let release.sh and this file drift on what "structural"
# means, which is the duplication the shared helper exists to remove.
# shellcheck source=scripts/lib/release_draft.sh
source "$DRAFT_LIB"

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
# The dispatch half, which is what the retry and the classifier are driven by.
# GH_DISPATCH_FAILS is how many LEADING attempts fail before one is queued, so
# a transient failure and a permanent one are the same stub with two numbers.
GH_DISPATCH_FAILS=0
GH_DISPATCH_ERR=""
# What the ref probe finds: present, absent, or unreadable. The third is its own
# answer, because a probe that cannot be read must not refuse a release.
# REF_ABSENT_PROBES makes the first N probes 404 whatever REF_STATE says, which
# is how a REST read racing our own force-push is modelled.
REF_STATE=present
REF_ABSENT_PROBES=0
# Which namespace the ref lives in. `heads` is an rc branch, `tags` is the
# v<version> shape --attach-notarized passes as both tag and ref.
REF_NAMESPACE=heads

# Retries must cost no wall-clock here. The ATTEMPT COUNT is left at the real
# default, so the suite exercises the shipped number rather than a test one.
# shellcheck disable=SC2034 # read by dispatch_dmg_verify, extracted at runtime
RELEASE_DMG_DISPATCH_BACKOFF_SECS=0

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
    api)      gh_api_stub "$@" ;;
    workflow) gh_workflow_stub ;;
    *)        return 0 ;;
  esac
}

# Two gh api shapes reach this stub, and they are opposites, so they are split
# by the DELETE rather than by the path: the tag-ref delete MUTATES, while the
# ref probe only reads. Conflating them made the probe consume RC_TAG_REF.
gh_api_stub() {
  case "$*" in
    *"-X DELETE"*)
      # A tag-ref delete 404s when no ref exists. That 404 is exactly what
      # --cleanup-tag turned into a failed command, so the stub reproduces it
      # rather than always succeeding.
      [ "$RC_TAG_REF" = 1 ] && { RC_TAG_REF=0; return 0; }
      return 1
      ;;
  esac
  # The heads call is logged first in every probe, so its running count IS the
  # 1-based probe index, and it does not move again while the tags call of the
  # same probe is answered.
  local probe
  probe="$(grep -c '/git/ref/heads/' "$GH_LOG")"
  if [ "$probe" -le "$REF_ABSENT_PROBES" ]; then
    printf 'gh: Not Found (HTTP 404)\n' >&2
    return 1
  fi
  case "$REF_STATE" in
    present)
      case "$*" in *"/git/ref/$REF_NAMESPACE/"*) return 0 ;; esac
      # The OTHER namespace answers exactly as GitHub does for a ref that is
      # not in it, so a tags-only ref still has to survive a heads 404.
      printf 'gh: Not Found (HTTP 404)\n' >&2
      return 1
      ;;
    absent)  printf 'gh: Not Found (HTTP 404)\n' >&2; return 1 ;;
    *)       printf 'error connecting to api.github.com\n' >&2; return 1 ;;
  esac
}

# The dispatch. It counts its own attempts out of the log rather than out of a
# variable: every subject runs inside run_fn's subshell, so a counter assigned
# here would be discarded, while the log is a file and survives.
gh_workflow_stub() {
  local attempts
  attempts="$(grep -c 'workflow run' "$GH_LOG")"
  if [ "$GH_DISPATCH_OK" = 1 ] && [ "$attempts" -gt "$GH_DISPATCH_FAILS" ]; then
    return 0
  fi
  [ -n "$GH_DISPATCH_ERR" ] && printf '%s\n' "$GH_DISPATCH_ERR" >&2
  return 1
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
  GH_DISPATCH_FAILS=0
  GH_DISPATCH_ERR=""
  REF_STATE=present
  REF_ABSENT_PROBES=0
  REF_NAMESPACE=heads
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
count_logged() { grep -cF -- "$1" "$GH_LOG" 2>/dev/null || true; }

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

# ── 11. a failed dispatch arrives WITH ITS REASON ─────────────────────────────
# The v0.39.1 defect. The dispatch sent gh's stderr to /dev/null, so the whole
# diagnosis of a failed gate was "Could not dispatch the DMG gate". The run
# aborted at the end of Phase A and the identical command worked from a shell
# seconds later, and nobody can say why.
echo
echo "test: a failed dispatch carries gh's own words to the operator"
new_case 0 0
GH_DISPATCH_OK=0
GH_DISPATCH_ERR='HTTP 503: Service Unavailable (api.github.com)'
run_fn dispatch_dmg_verify "$RC_TAG" "$RC_BRANCH"
assert_fail "a dispatch that never queues still returns non-zero" \
            "an unqueued dispatch returned 0"
case "$RUN_OUT" in
  *"HTTP 503: Service Unavailable"*) pass "gh's stderr reaches the operator, not /dev/null" ;;
  *)                                 fail_t "the reason was discarded: $RUN_OUT" ;;
esac

# Retrying is the point of the split, so the two classes must differ in COST as
# well as in exit code. 4 is the shipped default, asserted rather than
# overridden, so a change to it is a decision somebody takes on purpose.
echo
echo "test: a RETRYABLE failure is retried, a STRUCTURAL one is not"
if [ "$(count_logged 'workflow run')" -eq 4 ]; then
  pass "a retryable failure spends all four attempts"
else
  fail_t "expected 4 dispatch attempts, got $(count_logged 'workflow run')"
fi
if [ "$RUN_STATUS" = 1 ]; then
  pass "a retryable failure exits 1, the class that is worth a re-run"
else
  fail_t "a retryable failure exited $RUN_STATUS, not 1"
fi
new_case 0 0
GH_DISPATCH_OK=0
GH_DISPATCH_ERR='HTTP 422: Unexpected inputs provided: ["dmg_tag"]'
run_fn dispatch_dmg_verify "$RC_TAG" "$RC_BRANCH"
if [ "$RUN_STATUS" = 2 ]; then
  pass "a structural failure exits 2, the class no retry can fix"
else
  fail_t "a structural failure exited $RUN_STATUS, not 2: $RUN_OUT"
fi
if [ "$(count_logged 'workflow run')" -eq 1 ]; then
  pass "a structural failure is attempted exactly once"
else
  fail_t "a 422 cost $(count_logged 'workflow run') attempts; no retry can fix it"
fi

echo
echo "test: a retryable failure that clears on a later attempt still arms the gate"
# The v0.39.1 shape: a ref force-pushed moments earlier is not always resolvable
# by the Actions dispatch endpoint yet, and Phase A dispatches soon after the
# push. Two failures then a queued run must be a SUCCESS, not a dead release.
new_case 0 0
GH_DISPATCH_FAILS=2
GH_DISPATCH_ERR='HTTP 502: Bad Gateway'
run_fn dispatch_dmg_verify "$RC_TAG" "$RC_BRANCH"
assert_ok "the third attempt queues the gate" \
          "a transient failure killed a dispatch that would have worked"
if [ "$(count_logged 'workflow run')" -eq 3 ]; then
  pass "it stopped retrying the moment one was queued"
else
  fail_t "expected 3 attempts, got $(count_logged 'workflow run')"
fi
case "$RUN_OUT" in
  *"Retrying in"*) pass "the operator is told it is retrying, and why" ;;
  *)               fail_t "the retry was silent: $RUN_OUT" ;;
esac

echo
echo "test: a dispatch endpoint lagging behind the push is RETRIED, not called structural"
# The v0.39.1 shape end to end, and the one verdict the ref probe overrules.
# `No ref found for: <ref>` is a 422, so the shared classifier calls it
# structural. The probe already found the ref, so it is the Actions endpoint
# lagging a push made seconds ago, which is exactly what a retry clears.
new_case 0 0
GH_DISPATCH_FAILS=2
GH_DISPATCH_ERR="HTTP 422: No ref found for: $RC_BRANCH"
run_fn dispatch_dmg_verify "$RC_TAG" "$RC_BRANCH"
assert_ok "the lag cleared and the gate is armed" \
          "the retry never ran, so Phase A died on a lag that clears itself"
if [ "$(count_logged 'workflow run')" -eq 3 ]; then
  pass "it kept trying until the endpoint caught up"
else
  fail_t "expected 3 attempts, got $(count_logged 'workflow run')"
fi
case "$RUN_OUT" in
  *"does not yet"*) pass "the operator is told git has the ref and the endpoint does not" ;;
  *)                fail_t "the overrule was silent: $RUN_OUT" ;;
esac
# And it is still BOUNDED. A 422 naming the ref that never clears must end, and
# as a retryable failure, so the caller reads it as worth one more run.
new_case 0 0
GH_DISPATCH_OK=0
GH_DISPATCH_ERR="HTTP 422: No ref found for: $RC_BRANCH"
run_fn dispatch_dmg_verify "$RC_TAG" "$RC_BRANCH"
if [ "$RUN_STATUS" = 1 ] && [ "$(count_logged 'workflow run')" -eq 4 ]; then
  pass "a lag that never clears exits 1 after the four attempts, not forever"
else
  fail_t "exit $RUN_STATUS after $(count_logged 'workflow run') attempts: $RUN_OUT"
fi
# An UNCONFIRMED ref is the other half. The retries still run, because they are
# cheap, but the verdict does not become retryable: the probe never found the
# ref, so the dispatch saying it is missing is the only evidence there is, and
# the operator should go and fix the ref rather than re-run the release.
new_case 0 0
REF_STATE=error
GH_DISPATCH_OK=0
GH_DISPATCH_ERR="HTTP 422: No ref found for: $RC_BRANCH"
run_fn dispatch_dmg_verify "$RC_TAG" "$RC_BRANCH"
if [ "$RUN_STATUS" = 2 ] && [ "$(count_logged 'workflow run')" -eq 4 ]; then
  pass "an unreadable probe still tries, then reports the missing ref as structural"
else
  fail_t "exit $RUN_STATUS after $(count_logged 'workflow run') attempts: $RUN_OUT"
fi

# The overrule is NARROW. A 422 about anything else stays structural, so a
# workflow the mirror cannot accept is still refused on the first attempt.
new_case 0 0
GH_DISPATCH_OK=0
GH_DISPATCH_ERR='HTTP 422: Unexpected inputs provided: ["dmg_tag"]'
run_fn dispatch_dmg_verify "$RC_TAG" "$RC_BRANCH"
if [ "$RUN_STATUS" = 2 ] && [ "$(count_logged 'workflow run')" -eq 1 ]; then
  pass "a 422 that is not about the ref is still structural, and still costs one attempt"
else
  fail_t "the overrule widened: exit $RUN_STATUS after $(count_logged 'workflow run') attempts"
fi

echo
echo "test: an absent ref is reported as an absent ref, not as a dispatch failure"
# The honest 422 for this case is `No ref found for: rc/<version>`, which reads
# like a gh problem and is not one. Dispatching at a ref that is not there can
# only fail, so it is not attempted at all.
new_case 0 0
REF_STATE=absent
run_fn dispatch_dmg_verify "$RC_TAG" "$RC_BRANCH"
if [ "$RUN_STATUS" = 2 ]; then
  pass "an absent ref exits 2: no retry puts a ref on the mirror"
else
  fail_t "an absent ref exited $RUN_STATUS, not 2: $RUN_OUT"
fi
case "$RUN_OUT" in
  *"$RC_BRANCH does not exist"*) pass "the message names the ref that is missing" ;;
  *)                             fail_t "the message does not name the absent ref: $RUN_OUT" ;;
esac
assert_not_logged "workflow run" \
  "nothing is dispatched at a ref that is not there" \
  "a dispatch was attempted at an absent ref"

echo
echo "test: ONE 404 from the ref probe is a race with our own push, not proof"
# Phase A force-pushes rc/<version> and arms the gate seconds later. A REST read
# taken that soon can miss it, and believing the first answer would tell the
# operator to re-push a ref that is already there.
new_case 0 0
REF_ABSENT_PROBES=1
run_fn dispatch_dmg_verify "$RC_TAG" "$RC_BRANCH"
assert_ok "the re-probe found the ref and the gate was armed" \
          "one stale 404 killed Phase A over a ref that was already pushed"
assert_logged "workflow run install-smoke.yml" \
  "the dispatch went ahead once the probe caught up" \
  "nothing was dispatched after the ref appeared"

echo
echo "test: the probe reads the TAG namespace too, which is the --attach-notarized shape"
# That path passes v<version> as both the tag and the ref, so a probe that only
# ever asked heads/ would call every published tag an absent ref.
new_case 0 0
REF_NAMESPACE=tags
run_fn dispatch_dmg_verify "v$VERSION" "v$VERSION"
assert_ok "a ref that exists only as a tag is present" \
          "a tag ref was read as absent, so --attach-notarized could never re-verify"

echo
echo "test: a ref probe that cannot be READ never blocks the dispatch"
# The probe is a diagnostic. Refusing on an unreadable one would turn a blip on
# a read-only call into a failed Phase A, which is strictly worse than the 422
# it exists to explain.
new_case 0 0
REF_STATE=error
run_fn dispatch_dmg_verify "$RC_TAG" "$RC_BRANCH"
assert_ok "an unreadable probe still dispatches" \
          "a probe outage became a release outage"
assert_logged "workflow run install-smoke.yml" \
  "the dispatch went ahead on an unknown ref state" \
  "the dispatch was skipped because the probe could not be read"

echo
echo "test: Phase A's fatal carries the reason, not just the verdict"
# The whole chain: gh's stderr, through dispatch_dmg_verify, out of the caller
# that turns it into a fatal. That caller is what an operator actually sees.
new_case 0 0
GH_DISPATCH_OK=0
GH_DISPATCH_ERR='HTTP 403: Resource not accessible by integration'
run_fn refresh_release_candidate_draft
assert_fail "the arming step still dies on a gate it could not arm" \
            "a failed dispatch no longer fails Phase A"
case "$RUN_OUT" in
  *"HTTP 403: Resource not accessible"*)
    pass "the operator gets the diagnosis, not just 'could not dispatch'" ;;
  *)
    fail_t "the fatal arrived with no reason attached: $RUN_OUT" ;;
esac

# ── 12. the CI side of the same gate ──────────────────────────────────────────
workflow_assertions

summary
