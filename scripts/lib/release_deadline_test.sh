#!/usr/bin/env bash
# Tests for the notarization deadline: the pure parser in
# scripts/lib/release_deadline.sh, the poll change and the pending handoff in
# scripts/build-dmg.sh, and the phase rules in scripts/release.sh.
#
# WHAT THIS PINS, and why each half matters:
#   1. the parser accepts exactly three forms and refuses everything else,
#      including the shapes a caller is most likely to fat-finger (a bare number,
#      a negative duration, 25:00);
#   2. "the next 06:30" is resolved against an INJECTED clock, so the tomorrow
#      case is tested without waiting until tomorrow;
#   3. WITHOUT a deadline, notarize_poll still dies at NOTARIZE_POLL_TIMEOUT. The
#      whole change is additive, and this is the assertion that says so;
#   4. WITH one, expiry returns cleanly, and it governs INSTEAD of the timeout
#      (two bounds would mean the shorter wins, so a deadline further out than
#      the 7200s default would still die at 7200s);
#   5. the handoff exits with the pending code, leaves the resume handle, SUCCEEDS
#      the notarize cockpit step rather than failing it, and stages nothing. That
#      last one is the load-bearing safety property: with no staging dir there is
#      no manifest for --publish-verified to promote, so a run whose ticket never
#      arrived cannot publish an unstapled DMG;
#   6. both scripts refuse the flag where it cannot mean anything.
#
# build-dmg.sh is a script, not a library, so its functions are extracted with
# awk (the technique build_dmg_test.sh and release_staple_guard_test.sh already
# use) and driven against a fake notarytool. No xcrun, no network, no build, and
# no real release.
# Run: ./scripts/lib/release_deadline_test.sh
#
# shellcheck disable=SC2034 # file-wide: this suite drives functions EXTRACTED from
# build-dmg.sh, so most of its globals are written here and read only over there.
# A per-line suppression would be a dozen identical comments saying the same thing.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/release_deadline.sh
source "$SCRIPT_DIR/release_deadline.sh"
# release_notarize_json_field is what notarize_poll reads notarytool's reply
# with; it depends on release_staging_sha256, so that lib comes first.
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/release_staging.sh"
# shellcheck source=scripts/lib/release_notarize.sh
source "$SCRIPT_DIR/release_notarize.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# ── 1. The parser, against an injected clock ─────────────────────────────────
# 2026-08-06 12:00:00 local, whatever this machine's timezone is: the resolver
# takes the seconds-into-day as a parameter precisely so the test never has to
# know or set TZ.
NOW=1785931200
INTO_DAY=$((12 * 3600))

echo "test: a duration resolves to now + the duration"
for case_spec in "90m:5400" "2h:7200" "5400s:5400"; do
    spec="${case_spec%%:*}"
    want=$((NOW + ${case_spec##*:}))
    got="$(release_deadline_resolve "$spec" "$NOW" "$INTO_DAY" 2>&1)"
    if [ "$got" = "$want" ]; then
        pass "$spec resolves to $want"
    else
        fail "$spec resolved to '$got', wanted $want"
    fi
done

echo ""
echo "test: a wall-clock time resolves to the NEXT time it is that"
# 18:30 is later today, so it is today. 06:30 has already passed at noon, so it
# is tomorrow: the one case a caller actually relies on, since a nightly sets a
# morning deadline from the middle of the night.
if [ "$(release_deadline_resolve '18:30' "$NOW" "$INTO_DAY" 2>&1)" = "$((NOW + 6 * 3600 + 1800))" ]; then
    pass "18:30 from noon is later today"
else
    fail "18:30 from noon did not resolve to today"
fi
if [ "$(release_deadline_resolve '06:30' "$NOW" "$INTO_DAY" 2>&1)" = "$((NOW + 18 * 3600 + 1800))" ]; then
    pass "06:30 from noon is tomorrow"
else
    fail "06:30 from noon did not roll over to tomorrow"
fi
# Exactly now must roll over too, or a deadline set to the current minute would
# resolve into the past and expire before the first poll.
if [ "$(release_deadline_resolve '12:00' "$NOW" "$INTO_DAY" 2>&1)" = "$((NOW + 86400))" ]; then
    pass "12:00 at exactly 12:00:00 rolls over to tomorrow"
else
    fail "12:00 at exactly noon did not roll over"
fi
# A leading zero must not be read as octal: 08 and 09 are not valid octal at all.
if [ "$(release_deadline_resolve '08:09' "$NOW" "$INTO_DAY" 2>&1)" = "$((NOW + 20 * 3600 + 9 * 60))" ]; then
    pass "08:09 is parsed base 10, not octal"
else
    fail "08:09 was misparsed (octal?)"
fi

echo ""
echo "test: the @epoch plumbing form passes an absolute instant straight through"
if [ "$(release_deadline_resolve '@1785931500' "$NOW" "$INTO_DAY" 2>&1)" = "1785931500" ]; then
    pass "@1785931500 resolves to itself"
else
    fail "the @epoch form did not pass through"
fi

echo ""
echo "test: anything else is refused, naming the accepted forms"
for bad in "" "90" "90x" "-5m" "0m" "abc" "25:00" "12:60" "12:0" "1:2:3" "@" "@abc"; do
    if out="$(release_deadline_resolve "$bad" "$NOW" "$INTO_DAY" 2>&1)"; then
        fail "'$bad' was accepted, resolving to '$out'"
    elif echo "$out" | grep -q "ERROR"; then
        pass "'$bad' is refused"
    else
        fail "'$bad' failed without an explanation: $out"
    fi
done
# Every refusal has to say what WOULD work, or the caller is left guessing.
out="$(release_deadline_resolve 'nonsense' "$NOW" "$INTO_DAY" 2>&1 || true)"
if echo "$out" | grep -q '90m' && echo "$out" | grep -q '06:30'; then
    pass "the refusal names the accepted forms"
else
    fail "the refusal does not name the accepted forms: $out"
fi

echo ""
echo "test: an EMPTY deadline never expires (the no-flag path)"
if release_deadline_expired "" 9999999999; then
    fail "an empty deadline reported as expired, which would pause every release"
else
    pass "an empty deadline is never expired"
fi
if release_deadline_expired 100 100; then
    pass "a deadline exactly reached counts as expired"
else
    fail "a deadline exactly reached did not count as expired"
fi
if release_deadline_expired 200 100; then
    fail "a future deadline reported as expired"
else
    pass "a future deadline is not expired"
fi

# ── 2. notarize_poll, extracted from build-dmg.sh ────────────────────────────
echo ""
echo "test: extracting the poll + handoff from build-dmg.sh"
EXTRACT="$(mktemp)"
for fn in begin_step end_step die notarize_poll notarize_print_log \
          notarize_deadline_handoff notarize_await_verdict; do
    awk -v fn="$fn" '$0 ~ "^" fn "\\(\\) \\{",/^\}/' "$PROJECT_DIR/scripts/build-dmg.sh" >> "$EXTRACT"
    if ! grep -q "^$fn() {" "$EXTRACT"; then
        echo "  FAIL: could not extract $fn from build-dmg.sh"
        exit 1
    fi
done
# shellcheck source=/dev/null
source "$EXTRACT"
rm -f "$EXTRACT"
pass "extracted the poll, the handoff and the step helpers"

# The globals the extracted functions read. Poll interval 0 so the loop costs
# nothing except where a test deliberately wants a real second to pass.
RELEASE_MODE=1
CURRENT_STEP=""
EFFECTIVE_VERSION="0.77.0"   # synthetic: a fixture must never collide with a real release
REPO_ROOT="$PROJECT_DIR"
NOTARIZE_POLL_INTERVAL=0
NOTARIZE_POLL_TIMEOUT=7200
NOTARIZE_POLL_MAX_FAILURES=5
NOTARIZE_STATE_FILE=""
NOTARIZE_STATUS=""
NOTARIZE_DEADLINE=""
NOTARIZE_DEADLINE_EXPIRED=0
step() { :; }

# The cockpit events the extracted step helpers emit, recorded rather than sent.
EVENT_LOG="$(mktemp)"
emit_release_step() { printf '%s %s\n' "$1" "$2" >> "$EVENT_LOG"; }

# A fake notarytool. NOTARYTOOL_STATUS drives it, so one fake covers "never
# finishes", "Accepted" and "the transport is broken".
#
# The call COUNT goes in a file, not a variable: notarize_poll reads the reply
# through a command substitution, so every increment would otherwise happen in a
# subshell and be discarded the moment it mattered.
NOTARYTOOL_STATUS="In Progress"
NOTARYTOOL_CALLS_FILE="$(mktemp)"
notarytool_calls() { wc -l < "$NOTARYTOOL_CALLS_FILE" | tr -d ' '; }
notarytool_run() {
    echo "call" >> "$NOTARYTOOL_CALLS_FILE"
    [ "$NOTARYTOOL_STATUS" != "BROKEN" ] || { echo "fake transport failure" >&2; return 1; }
    printf '{"id":"%s","status":"%s"}\n' "${2:-none}" "$NOTARYTOOL_STATUS"
}

SUBMISSION="11111111-2222-3333-4444-555555555555"

echo ""
echo "test: WITHOUT a deadline the poll still dies at NOTARIZE_POLL_TIMEOUT"
NOTARIZE_DEADLINE=""
NOTARIZE_POLL_TIMEOUT=0
NOTARYTOOL_STATUS="In Progress"
out="$( (notarize_poll "$SUBMISSION") 2>&1 )"; rc=$?
if [ "$rc" -ne 0 ] && echo "$out" | grep -q "still In Progress"; then
    pass "the timeout still kills an endless wait when no deadline was given"
else
    fail "the no-deadline path stopped dying on timeout (rc=$rc): $out"
fi

echo ""
echo "test: WITH a deadline already past, the poll returns cleanly and asks Apple nothing"
NOTARIZE_POLL_TIMEOUT=7200
NOTARIZE_DEADLINE="$(( $(date '+%s') - 60 ))"
NOTARIZE_DEADLINE_EXPIRED=0
NOTARIZE_STATUS="unset"
: > "$NOTARYTOOL_CALLS_FILE"
if notarize_poll "$SUBMISSION" >/dev/null 2>&1; then
    pass "the poll returns 0 rather than dying"
else
    fail "the poll did not return 0 at an expired deadline"
fi
if [ "$NOTARIZE_DEADLINE_EXPIRED" = "1" ]; then
    pass "it flags the expiry for the caller"
else
    fail "NOTARIZE_DEADLINE_EXPIRED was not set"
fi
if [ -z "$NOTARIZE_STATUS" ]; then
    pass "it reports NO verdict, so nothing downstream can read one"
else
    fail "a verdict was invented at the deadline: '$NOTARIZE_STATUS'"
fi
if [ "$(notarytool_calls)" -eq 0 ]; then
    pass "an already-past deadline spends no round-trip to Apple"
else
    fail "the poll called notarytool $(notarytool_calls) time(s) despite a past deadline"
fi

echo ""
echo "test: the deadline governs INSTEAD of NOTARIZE_POLL_TIMEOUT, not beside it"
# The timeout is zero, so the old code would die on the first iteration. With a
# deadline set, the deadline is the only bound: this is what lets an operator
# ask for a wait LONGER than the 7200s default.
NOTARIZE_POLL_TIMEOUT=0
NOTARIZE_DEADLINE="$(( $(date '+%s') + 1 ))"
NOTARIZE_DEADLINE_EXPIRED=0
NOTARIZE_POLL_INTERVAL=1
NOTARYTOOL_STATUS="In Progress"
: > "$NOTARYTOOL_CALLS_FILE"
if notarize_poll "$SUBMISSION" >/dev/null 2>&1 && [ "$NOTARIZE_DEADLINE_EXPIRED" = "1" ]; then
    pass "a zero timeout no longer kills the wait once a deadline is set"
else
    fail "NOTARIZE_POLL_TIMEOUT still fired with a deadline in force"
fi
if [ "$(notarytool_calls)" -ge 1 ]; then
    pass "it polled at least once before the deadline came round"
else
    fail "the loop never polled, so this proved nothing"
fi
NOTARIZE_POLL_INTERVAL=0

echo ""
echo "test: a real verdict still wins while a deadline is pending"
NOTARIZE_POLL_TIMEOUT=7200
NOTARIZE_DEADLINE="$(( $(date '+%s') + 3600 ))"
NOTARIZE_DEADLINE_EXPIRED=0
NOTARIZE_STATUS=""
NOTARYTOOL_STATUS="Accepted"
if notarize_poll "$SUBMISSION" >/dev/null 2>&1 \
   && [ "$NOTARIZE_STATUS" = "Accepted" ] && [ "$NOTARIZE_DEADLINE_EXPIRED" = "0" ]; then
    pass "an Accepted verdict inside the deadline is reported as Accepted"
else
    fail "a verdict inside the deadline was mishandled (status='$NOTARIZE_STATUS' expired=$NOTARIZE_DEADLINE_EXPIRED)"
fi

# ── 3. The pending handoff ───────────────────────────────────────────────────
# A tree carrying the two things a deadline expiry must leave behind (the resume
# handle) and the one it must NOT create (a staging manifest).
new_pending_tree() {
    local root
    root="$(mktemp -d)"
    mkdir -p "$root/.lucidos/release-state" "$root/.lucidos/release-staging/$EFFECTIVE_VERSION"
    printf '{"stage":"dmg","submission_id":"%s"}\n' "$SUBMISSION" \
        > "$root/.lucidos/release-state/notarize-$EFFECTIVE_VERSION.json"
    printf '%s' "$root"
}

echo ""
echo "test: a deadline expiry exits with the pending code, not a failure"
TREE="$(new_pending_tree)"
NOTARIZE_STATE_FILE="$TREE/.lucidos/release-state/notarize-$EFFECTIVE_VERSION.json"
REPO_ROOT="$TREE"
NOTARIZE_DEADLINE="$(( $(date '+%s') - 60 ))"
NOTARIZE_DEADLINE_EXPIRED=0
NOTARIZE_STATUS=""
NOTARYTOOL_STATUS="In Progress"
: > "$EVENT_LOG"
CURRENT_STEP="notarize"
out="$( (notarize_await_verdict "$SUBMISSION") 2>&1 )"; rc=$?
if [ "$rc" -eq "$RELEASE_NOTARY_PENDING_EXIT" ]; then
    pass "it exits $RELEASE_NOTARY_PENDING_EXIT (the pending code), not 0 and not 1"
else
    fail "expected exit $RELEASE_NOTARY_PENDING_EXIT, got $rc: $out"
fi
if echo "$out" | grep -q "NOTARY PENDING"; then
    pass "it prints a clearly-marked NOTARY PENDING block"
else
    fail "no NOTARY PENDING block: $out"
fi
if echo "$out" | grep -q "$SUBMISSION"; then
    pass "the block names the submission id"
else
    fail "the block does not name the submission id: $out"
fi
if echo "$out" | grep -q -- "--resume-notarize $EFFECTIVE_VERSION"; then
    pass "the block names the exact resume command"
else
    fail "the block does not name the resume command: $out"
fi
if echo "$out" | grep -qi "not a failure"; then
    pass "the block says out loud that this is not a failure"
else
    fail "the block reads like an error report: $out"
fi

echo ""
echo "test: the expiry leaves the resume handle and stages NOTHING"
if [ -f "$NOTARIZE_STATE_FILE" ]; then
    pass "the notarize resume handle survives"
else
    fail "the resume handle was removed, so the submission is unrecoverable"
fi
if [ ! -f "$TREE/.lucidos/release-staging/$EFFECTIVE_VERSION/manifest.json" ]; then
    pass "no staging manifest was written"
else
    fail "a staging manifest exists, so --publish-verified could promote an unstapled DMG"
fi
# The strongest form of the same claim: whatever is in that dir, the question
# every publish path asks about it must not answer "notarized".
if release_staging_is_notarized "$TREE/.lucidos/release-staging/$EFFECTIVE_VERSION" 2>/dev/null; then
    fail "the staging dir reads as NOTARIZED after a deadline expiry"
else
    pass "the staging dir does not read as notarized"
fi

echo ""
echo "test: the expiry SUCCEEDS the notarize step and emits no ReleaseStepFailed"
if grep -q '^Succeeded notarize$' "$EVENT_LOG"; then
    pass "the notarize cockpit step is closed as Succeeded"
else
    fail "the notarize step was not succeeded: $(cat "$EVENT_LOG")"
fi
if grep -q '^Failed' "$EVENT_LOG"; then
    fail "a ReleaseStepFailed was emitted for a deadline expiry: $(cat "$EVENT_LOG")"
else
    pass "no ReleaseStepFailed for a pause the operator asked for"
fi
rm -rf "$TREE"
rm -f "$EVENT_LOG" "$NOTARYTOOL_CALLS_FILE"

# ── 4. The flag rules in build-dmg.sh ────────────────────────────────────────
BUILD_DMG="$PROJECT_DIR/scripts/build-dmg.sh"

echo ""
echo "test: build-dmg.sh refuses a deadline it cannot honour"
out="$("$BUILD_DMG" --notarize-deadline nonsense --release-build 2>&1)"; RC=$?
if [ "$RC" -ne 0 ] && echo "$out" | grep -q '90m'; then
    pass "a bad spec is refused, naming the accepted forms"
else
    fail "a bad spec was accepted (rc=$RC): $out"
fi
out="$("$BUILD_DMG" --notarize-deadline 90m 2>&1)"; RC=$?
if [ "$RC" -ne 0 ] && echo "$out" | grep -q "only applies to a build"; then
    pass "a deadline with no build mode is refused"
else
    fail "a deadline outside a build was accepted (rc=$RC): $out"
fi
out="$("$BUILD_DMG" --notarize-deadline 90m --release-attach --staging-dir /nonexistent --upload-tag v0.0.0 2>&1)"; RC=$?
if [ "$RC" -ne 0 ] && echo "$out" | grep -q "cannot be combined with --release"; then
    pass "a deadline on --release-attach is refused"
else
    fail "a deadline on the attach path was accepted (rc=$RC): $out"
fi
out="$("$BUILD_DMG" --notarize-deadline 90m --release-build --defer-notarization 2>&1)"; RC=$?
if [ "$RC" -ne 0 ] && echo "$out" | grep -q "alternatives, not a pair"; then
    pass "a deadline alongside --defer-notarization is refused"
else
    fail "deadline + defer was accepted (rc=$RC): $out"
fi

# ── 5. The phase rules in release.sh ─────────────────────────────────────────
RELEASE_SH="$PROJECT_DIR/scripts/release.sh"
echo ""
if [ ! -f "$RELEASE_SH" ]; then
    echo "  skip: release.sh is not present (stripped from the public mirror)"
else
    echo "test: release.sh accepts the deadline only where something waits on Apple"
    # 0.77.0 is synthetic and has no state on disk, so every one of these refusals
    # is reached long before anything could be created, pushed or published.
    out="$("$RELEASE_SH" --notarize-deadline nonsense --verify-build 0.77.0 2>&1)"; RC=$?
    if [ "$RC" -ne 0 ] && echo "$out" | grep -q '5400s'; then
        pass "a bad spec is refused up front, naming the accepted forms"
    else
        fail "release.sh accepted a bad deadline spec (rc=$RC): $out"
    fi
    for phase in --publish-verified --push-rc --publish-draft --attach-notarized; do
        out="$("$RELEASE_SH" --notarize-deadline 90m "$phase" 0.77.0 2>&1)"; RC=$?
        if [ "$RC" -ne 0 ] && echo "$out" | grep -q "does not apply to $phase"; then
            pass "$phase refuses a deadline"
        else
            fail "$phase accepted a deadline (rc=$RC): $out"
        fi
    done
    out="$("$RELEASE_SH" --notarize-deadline 90m 0.77.0 2>&1)"; RC=$?
    if [ "$RC" -ne 0 ] && echo "$out" | grep -q "needs the two-phase flow"; then
        pass "the one-shot refuses a deadline and says which flow to use"
    else
        fail "the one-shot accepted a deadline (rc=$RC): $out"
    fi
    out="$("$RELEASE_SH" -c /nonexistent-changelog.md --notarize-deadline 90m --defer-notarization --verify-build 0.77.0 2>&1)"; RC=$?
    if [ "$RC" -ne 0 ] && echo "$out" | grep -q "alternatives, not a pair"; then
        pass "deadline + defer is refused, with the difference explained"
    else
        fail "release.sh accepted deadline + defer (rc=$RC): $out"
    fi

    echo ""
    echo "test: release.sh hands build-dmg.sh an already-resolved instant"
    # Passing the operator's spec through verbatim would let a duration
    # re-anchor itself at whatever moment build-dmg.sh parsed it, so a 90m
    # deadline would mean 90 minutes after a 40-minute build.
    # shellcheck disable=SC2016 # \$NOTARIZE_DEADLINE is the literal text being searched for
    if grep -q 'BUILD_DEADLINE_ARGS=(--notarize-deadline "@$NOTARIZE_DEADLINE")' "$RELEASE_SH"; then
        pass "the deadline goes down as the @epoch form, resolved once"
    else
        fail "release.sh does not hand down a resolved deadline"
    fi
    if grep -q 'BUILD_DEADLINE_ARGS\[@\]' "$RELEASE_SH"; then
        pass "the resolved deadline reaches the build invocation"
    else
        fail "BUILD_DEADLINE_ARGS is never expanded into a build"
    fi

    echo ""
    echo "test: the pending path writes the verify-build state and prints its own handoff"
    # write_verify_build_state must already have run BEFORE the build in both
    # Phase-A entry points, or a deadline expiry would strand a worktree that
    # neither --resume-notarize nor --publish-verified could pick up.
    body="$(awk '/^run_resume_notarize\(\) \{/,/^\}/' "$RELEASE_SH")"
    state_line="$(printf '%s\n' "$body" | grep -n 'write_verify_build_state' | head -1 | cut -d: -f1)"
    build_line="$(printf '%s\n' "$body" | grep -n 'run_release_build_dmg' | head -1 | cut -d: -f1)"
    if [ -n "$state_line" ] && [ -n "$build_line" ] && [ "$state_line" -lt "$build_line" ]; then
        pass "the resume writes the verify-build state before the build"
    else
        fail "the resume must persist its state before the build (state=$state_line build=$build_line)"
    fi
    if grep -q 'print_notary_pending_handoff' "$RELEASE_SH"; then
        pass "a pending run gets its own handoff block"
    else
        fail "nothing prints a NOTARY PENDING handoff"
    fi
    # The verify-first handoff claims a staged, verified build and points at
    # --publish-verified. Printing it for a run that staged nothing would send an
    # operator to promote artifacts that do not exist.
    if awk '/^cleanup\(\) \{/,/^\}/' "$RELEASE_SH" \
         | grep -q 'NOTARY_PENDING'; then
        pass "cleanup distinguishes a paused run from a completed Phase A"
    else
        fail "cleanup would print the completed-Phase-A handoff for a paused run"
    fi
fi

echo ""
echo "release_deadline: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
