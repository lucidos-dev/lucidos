#!/bin/bash
# Tests for the two front-door jobs in .github/workflows/install-smoke.yml, the
# ones that run the advertised `curl … | sh` against a live origin.
#
# The subject is the WORKFLOW FILE, not a shell library, and that is what makes
# this a drift test rather than a unit test: the jobs only ever execute in the
# public mirror, so nothing else in this repo can exercise them before a release
# does. Everything asserted here is checkable offline from the file itself.
#
# It is also the guard against the standing hazard of that file: `front-door`
# and `front-door-macos` are duplicated DELIBERATELY, so each reports
# independently, which means a fix can silently land in one and not the other.
# Every assertion below therefore runs ONCE PER JOB.
#
# What this pins, all of it traceable to
# docs/plans/2026-07-31-front-door-asset-preflight-and-rc-dispatch.md:
#   1. the asset preflight exists, is full-mode-only, and is ordered BEFORE the
#      launch step, so the install never starts against a release whose tarball
#      has not been attached yet;
#   2. its wait is BOUNDED and its poll interval is in the 30 to 60 s band;
#   3. expiry fails immediately, naming the asset URL and the release-tarballs
#      window, instead of falling through into the 900 s gateway poll;
#   4. every parse in the preflight fails CLOSED, and the URL it probes is the
#      one install.sh will really fetch (cross-checked against the tree's own
#      install_common.sh + headless_tarball.sh);
#   5. a download failure AFTER a green preflight is reported as a download
#      failure, from inside the health poll, not as a health timeout;
#   6. the budgets fit inside timeout-minutes, so no run is killed with no
#      diagnosis;
#   7. an RC origin on anything but the rc/** push is REFUSED, before any fetch,
#      and is never downgraded to payload mode instead;
#   8. a Cloudflare Access login page is told apart from a Pages soft 404;
#   9. no existing guard is weakened: the public-mirror-only `if`, the empty
#      `permissions:`, the hostile-origin allowlist and its FRONT_DOOR_INPUT to
#      FRONT_DOOR rename, and the macOS rule that an exited installer is never
#      by itself a verdict.
#
# And one more, from the v0.18.5 propagation failure rather than that plan:
#  10. an rc/** version mismatch is RE-READ before it is believed, with a
#      bounded budget, a cache-busting fetch that can actually see past the
#      runner's Cloudflare POP, and no widening of the retry to any other
#      verdict in the step. FD_SERVED_VERSION must be exported after that loop,
#      or a retried version would be shadowed by the stale first read.
#
# And one more, from the v0.20.1 post-publish drift:
#  11. when the caller names the release it dispatched the run to verify, the
#      job's TWO fetches of install.sh (rung 1's, and the install step's
#      independent re-fetch) are pinned to that ONE release: rung 1 converges on
#      expect_version, bounded, before exporting FD_SERVED_VERSION, and a
#      download failure naming a different version is reported as drift rather
#      than as a flat download failure.
#
#      Its load-bearing half is a NEGATIVE, which is why it is pinned rather
#      than left to the comment beside it: this loop polls the PLAIN url, with
#      no nonce and no cache headers, exactly opposite to the rc loop in item 10.
#      A '?cb=' query string is a different Cloudflare cache key, so converging
#      on a nonced URL would prove nothing about the plain one the install step
#      fetches. The contrast reads like an oversight and invites a "fix", so
#      both halves are asserted: nonce present up there, absent down here.
#
# Hermetic and offline: it reads one tracked file and two tracked libs, makes no
# network call, and runs no part of the workflow. Comment lines are stripped
# before every assertion, so a job's own prose about a rule (the macOS job
# documents the ABSENCE of a `kill -0` fast-fail in words) can neither satisfy
# nor violate one.
#
# Its sibling scripts/lib/front_door_parity_test.sh has a different subject: it
# exercises the front_door_parity.sh harness against two file:// origins. The
# two do not overlap and neither can substitute for the other.
#
# Run: ./scripts/lib/front_door_gate_test.sh

# shellcheck disable=SC2016 # file-wide, and a genuine false positive throughout:
# every needle here is LITERAL text to find in the workflow, so the '$' in
# `"$INSTALL_PID"`, `$tarball_url`, `$RUNNER_TEMP/front-door-payloads` and their
# neighbours must reach grep UNEXPANDED. Expanding any of them (they are unset
# here) would turn the needle into the empty string and the assertion into a
# vacuous pass, which is the one failure mode a drift test cannot afford.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
WORKFLOW="$PROJECT_DIR/.github/workflows/install-smoke.yml"

# The slack the job needs for everything that is NOT one of the two budgets:
# apt/runner setup, the payload sniff, the real install (download, extract,
# embedded-PG initdb) and the four uninstall rungs. Generous on purpose, since
# the assertion it feeds is "the ceiling exceeds what the job can spend".
SLACK_SECS=900

JOBS=(front-door front-door-macos)

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

[ -r "$WORKFLOW" ] || { echo "ERROR: cannot read $WORKFLOW" >&2; exit 1; }

# ── extraction ────────────────────────────────────────────────────────────────
# Every helper emits "<line-number>:<text>", so an assertion can talk about
# ORDER as well as presence. Ordering is half of what this file checks: a
# preflight after the launch step, or a refusal after the first fetch, would
# satisfy a bare presence test while doing nothing.

# job_block <job-key>: the job's lines, up to the next sibling job key.
job_block() {
  awk -v key="  $1:" '
    $0 == key   { inblk = 1; print NR ":" $0; next }
    inblk && /^  [A-Za-z0-9_-]+:[[:space:]]*$/ { exit }
    inblk       { print NR ":" $0 }
  ' "$WORKFLOW"
}

# job_code <job-key>: the same, with comment-only lines removed.
job_code() {
  job_block "$1" | grep -vE '^[0-9]+:[[:space:]]*#'
}

# step_code <job-key> <step-name-fragment>: one step's code lines.
step_code() {
  job_code "$1" | awk -v frag="$2" '
    index($0, "- name: ") && index($0, frag) { inblk = 1; print; next }
    inblk && index($0, "- name: ")           { exit }
    inblk                                    { print }
  '
}

# line_with <fixed-string>: the first matching line number on stdin, or empty.
line_with() { grep -F -m1 -- "$1" | cut -d: -f1; }

# guarded_block <guard-needle>: on stdin a job's payload-sniff step, on stdout
# ONLY the block that guard opens, from its own line to the matching `fi`.
#
# Two assertions need block SCOPE rather than a line number, and both would be
# vacuous without it:
#
#   * the sniff step holds two re-read loops whose cache-busting rules are
#     OPPOSITE, so a search over the whole step cannot tell which loop it
#     matched, and a stray `cb=` from the rc loop would satisfy the "no nonce
#     here" check without proving anything about the other loop;
#   * "FD_SERVED_VERSION is exported after the loop" has to mean after the loop
#     ENDS. Compared against the `while` line instead, an export moved INSIDE
#     the body still passes while pinning whatever intermediate read the loop
#     happened to be holding, which is the exact bug both loops exist to avoid.
#
# The terminator is a `fi` at the guard's own indentation (ten spaces, the depth
# every `run:` body sits at); the nested `fi`s and the `done` are deeper.
guarded_block() {
  awk -v needle="$1" '
    index($0, needle)                    { inblk = 1; print; next }
    inblk && /^[0-9]+:          fi[ ]*$/ { print; exit }
    inblk                                { print }
  '
}

# block_end: on stdin a guarded_block, on stdout the line number of its `fi`.
block_end() { tail -n 1 | cut -d: -f1; }

# ── per-job assertions ────────────────────────────────────────────────────────
for job in "${JOBS[@]}"; do
  echo
  echo "$job"

  block="$(job_block "$job")"
  if [ -z "$block" ]; then
    fail "job '$job' not found in $WORKFLOW (every assertion below is skipped)"
    continue
  fi
  code="$(job_code "$job")"
  preflight="$(step_code "$job" "Wait for the release assets")"
  verify="$(step_code "$job" "Verify the front door installed a working Lucidos")"

  # ── 1. the preflight exists, is full-mode-only, and precedes the launch ─────
  pre_line="$(printf '%s\n' "$code" | line_with "- name: Wait for the release assets")"
  run_line="$(printf '%s\n' "$code" | line_with "- name: Run the advertised command")"
  if [ -z "$pre_line" ]; then
    fail "no asset-preflight step: the install would start before its tarball is published"
  elif [ -z "$run_line" ]; then
    fail "no launch step, so the preflight ordering cannot be checked"
  elif [ "$pre_line" -lt "$run_line" ]; then
    pass "the asset preflight (line $pre_line) precedes the launch step (line $run_line)"
  else
    fail "the asset preflight (line $pre_line) does NOT precede the launch step (line $run_line)"
  fi

  if printf '%s\n' "$preflight" | grep -qF "if: env.FD_MODE == 'full'"; then
    pass "the preflight is full-mode only (an RC has no tarball to wait for)"
  else
    fail "the preflight is not gated on FD_MODE == 'full', so payload mode would wait for an asset that cannot exist"
  fi

  # ── 2. the wait is bounded, the poll is in the 30 to 60 s band ──────────────
  wait_secs="$(printf '%s\n' "$code" | sed -n "s/^[0-9]*:[[:space:]]*FD_ASSET_WAIT_SECS:[[:space:]]*'\([0-9]*\)'.*/\1/p" | head -n 1)"
  poll_secs="$(printf '%s\n' "$code" | sed -n "s/^[0-9]*:[[:space:]]*FD_ASSET_POLL_SECS:[[:space:]]*'\([0-9]*\)'.*/\1/p" | head -n 1)"
  health_secs="$(printf '%s\n' "$code" | sed -n "s/^[0-9]*:[[:space:]]*GW_HEALTH_TIMEOUT_SECS:[[:space:]]*'\([0-9]*\)'.*/\1/p" | head -n 1)"
  timeout_min="$(printf '%s\n' "$code" | sed -n 's/^[0-9]*:[[:space:]]*timeout-minutes:[[:space:]]*\([0-9]*\).*/\1/p' | head -n 1)"
  # Parsed here rather than in section 11, because the ceiling arithmetic in
  # section 6 needs it: this budget can STACK on the preflight's.
  ev_wait="$(printf '%s\n' "$code" | sed -n "s/^[0-9]*:[[:space:]]*FD_EXPECT_VERSION_WAIT_SECS:[[:space:]]*'\([0-9]*\)'.*/\1/p" | head -n 1)"
  ev_poll="$(printf '%s\n' "$code" | sed -n "s/^[0-9]*:[[:space:]]*FD_EXPECT_VERSION_POLL_SECS:[[:space:]]*'\([0-9]*\)'.*/\1/p" | head -n 1)"

  if [ -n "$wait_secs" ] && [ -n "$poll_secs" ]; then
    if [ "$poll_secs" -gt 0 ] && [ "$poll_secs" -le 60 ] && [ "$wait_secs" -ge 60 ] && [ "$wait_secs" -le 3600 ]; then
      pass "the preflight budget is bounded and literal: wait ${wait_secs}s, poll ${poll_secs}s"
    else
      fail "the preflight budget is out of band: wait ${wait_secs}s, poll ${poll_secs}s (want 0 < poll <= 60 <= wait <= 3600)"
    fi
  else
    fail "FD_ASSET_WAIT_SECS / FD_ASSET_POLL_SECS are not both literal integers in this job's env (an unbounded or interpolated wait is exactly what this asserts against)"
  fi

  # ── 3. expiry is immediate, and says what is missing ────────────────────────
  expiry="$(printf '%s\n' "$preflight" | grep -F 'ASSET AVAILABILITY' | head -n 1)"
  if [ -z "$expiry" ]; then
    fail "the preflight has no expiry error naming the failure as an asset-availability one"
  else
    exp_line="${expiry%%:*}"
    ok=1
    case "$expiry" in *'$tarball_url'*) ;; *) ok=0; fail "the expiry error does not name the tarball URL" ;; esac
    case "$expiry" in *'release-tarballs'*) ;; *) ok=0; fail "the expiry error does not name release-tarballs, which is the window that explains it" ;; esac
    next="$(sed -n "$((exp_line + 1))p" "$WORKFLOW")"
    case "$next" in
      *'exit 1'*) ;;
      *) ok=0; fail "the expiry error is not immediately followed by 'exit 1', so the job would fall through into the gateway poll" ;;
    esac
    [ "$ok" -eq 1 ] && pass "expiry fails fast, naming the tarball URL and the release-tarballs window"
  fi

  # ── 4. every parse in the preflight fails closed ────────────────────────────
  closed_ok=1
  for frag in \
    'FD_SERVED_VERSION is empty' \
    'left no served install_common.sh' \
    "is not the expected 'https://...%s' shape" \
    "not the 'lucidos-%s-%s' this preflight constructs"
  do
    printf '%s\n' "$preflight" | grep -qF -- "$frag" && continue
    closed_ok=0
    fail "the preflight is missing its fail-closed branch for: $frag"
  done
  [ "$closed_ok" -eq 1 ] && pass "every preflight parse fails closed (version, served lib, base URL shape, artifact stem)"

  # The version the preflight probes must come from the SERVED installer, which
  # is what a piped run resolves, and rung 1 must hand it over rather than the
  # preflight re-fetching the origin and possibly getting a different answer.
  if printf '%s\n' "$code" | grep -qF 'FD_SERVED_VERSION=$served_version' \
     && printf '%s\n' "$code" | grep -qF 'tmp="$RUNNER_TEMP/front-door-payloads"' \
     && printf '%s\n' "$preflight" | grep -qF 'payloads="$RUNNER_TEMP/front-door-payloads"'; then
    pass "the preflight consumes rung 1's served payloads and its parsed version"
  else
    fail "rung 1 does not hand its payloads dir + parsed version to the preflight, so the two can disagree about what the origin serves"
  fi

  # ── 5. a residual download failure is named, from inside the poll ───────────
  if printf '%s\n' "$verify" | grep -qF 'assert_no_download_failure()'; then
    pass "the health poll defines assert_no_download_failure"
  else
    fail "the health poll has no assert_no_download_failure, so a post-preflight 404 reports as a gateway timeout"
  fi
  loop_line="$(printf '%s\n' "$verify" | line_with '-lt "$deadline"')"
  done_line="$(printf '%s\n' "$verify" | grep -E '^[0-9]+:[[:space:]]*done[[:space:]]*$' | head -n 1 | cut -d: -f1)"
  call_line="$(printf '%s\n' "$verify" | grep -E '^[0-9]+:[[:space:]]*assert_no_download_failure[[:space:]]*$' | head -n 1 | cut -d: -f1)"
  if [ -n "$loop_line" ] && [ -n "$done_line" ] && [ -n "$call_line" ] \
     && [ "$call_line" -gt "$loop_line" ] && [ "$call_line" -lt "$done_line" ]; then
    pass "assert_no_download_failure is called INSIDE the health poll (line $call_line, loop $loop_line to $done_line)"
  else
    fail "assert_no_download_failure is not called inside the health poll, so the job would still burn the whole health budget first"
  fi

  # ── 6. the budgets fit inside the ceiling ───────────────────────────────────
  # All THREE waits are counted, because the expect_version convergence can
  # genuinely stack on the preflight: a POP still serving the previous release
  # is exactly a POP whose new release is fresh enough for release-tarballs.yml
  # to still be attaching its assets, so one run can spend both budgets in a row.
  if [ -n "$wait_secs" ] && [ -n "$health_secs" ] && [ -n "$timeout_min" ] && [ -n "$ev_wait" ]; then
    needed=$((ev_wait + wait_secs + health_secs + SLACK_SECS))
    ceiling=$((timeout_min * 60))
    if [ "$needed" -le "$ceiling" ]; then
      pass "the budgets fit: ${ev_wait}s convergence + ${wait_secs}s preflight + ${health_secs}s health + ${SLACK_SECS}s slack = ${needed}s <= ${ceiling}s (timeout-minutes: $timeout_min)"
    else
      fail "the budgets do NOT fit: ${needed}s needed but timeout-minutes is $timeout_min (${ceiling}s). A job timeout is a hard kill with no diagnosis."
    fi
  else
    fail "cannot do the timeout arithmetic: one of FD_EXPECT_VERSION_WAIT_SECS / FD_ASSET_WAIT_SECS / GW_HEALTH_TIMEOUT_SECS / timeout-minutes is not a literal"
  fi

  # ── 7. an RC origin is refused, before any fetch, and never downgraded ──────
  refuse_line="$(printf '%s\n' "$code" | line_with "The RC front door is verified by THIS workflow's 'push: rc/**' arm")"
  curl_line="$(printf '%s\n' "$code" | line_with 'curl -')"
  if [ -z "$refuse_line" ]; then
    fail "no RC-origin refusal: a dispatch naming the RC origin would run against an origin that has no release tag"
  elif [ -n "$curl_line" ] && [ "$refuse_line" -gt "$curl_line" ]; then
    fail "the RC refusal (line $refuse_line) comes AFTER the job's first fetch (line $curl_line); it must refuse up front"
  else
    pass "an RC origin is refused at line $refuse_line, before the first fetch (line ${curl_line:-none})"
  fi

  if printf '%s\n' "$code" | grep -qF '[ "$fd_is_rc" -eq 1 ] && [ "${GITHUB_EVENT_NAME:-}" != "push" ]'; then
    pass "the refusal is scoped to non-push events, leaving the rc/** arm as the RC leg's owner"
  else
    fail "the RC refusal is not scoped to non-push events, so it would also refuse the rc/** push that legitimately gates the RC"
  fi

  if printf '%s\n' "$code" | grep -qF 'https://rc.*|*/rc)'; then
    pass "the RC classification matches both the host form and the legacy path form"
  else
    fail "the RC classification does not match both https://rc.* and the legacy .../rc path form"
  fi

  if printf '%s\n' "$code" | grep -qE 'FD_MODE=["'"'"']?payload'; then
    fail "the job assigns FD_MODE=payload somewhere: the downgrade was REVERSED on 2026-07-31 and an RC dispatch must fail, not be quietly demoted"
  else
    pass "no RC downgrade: the job never rewrites FD_MODE to payload"
  fi

  if printf '%s\n' "$code" | grep -qF '[ "$fd_is_rc" -eq 1 ] && [ "$FD_MODE" != "payload" ]'; then
    pass "the fail-closed companion holds: an RC origin outside payload mode is refused"
  else
    fail "nothing refuses an RC origin whose FD_MODE is not payload, so an edit to the job-level expression could drive a full install at the RC origin"
  fi

  # ── 8. an Access login page is not a soft 404 ───────────────────────────────
  if printf '%s\n' "$code" | grep -qF '%{url_effective}' \
     && printf '%s\n' "$code" | grep -qF '*cloudflareaccess.com*)'; then
    pass "a Cloudflare Access login page is reported as an auth failure, not a soft 404"
  else
    fail "the payload sniff cannot tell an Access login page from a Pages soft 404 (it needs the effective URL of the installer fetch)"
  fi

  # ── 9. nothing already guarding this job is weakened ────────────────────────
  guards_ok=1
  for frag in \
    'permissions: {}' \
    "github.repository == 'lucidos-dev/lucidos'" \
    'FRONT_DOOR_INPUT:' \
    'echo "FRONT_DOOR=$origin" >> "$GITHUB_ENV"' \
    '^https://[A-Za-z0-9]'
  do
    printf '%s\n' "$code" | grep -qF -- "$frag" && continue
    guards_ok=0
    fail "an existing guard is missing: $frag"
  done
  [ "$guards_ok" -eq 1 ] && pass "the existing guards are intact (mirror-only, no permissions, hostile-origin allowlist, validated rename)"

  # ── 10. an rc/** version mismatch is re-read before it is believed ──────────
  # release.sh arms this gate by blocking until the RC origin serves the
  # candidate, but that wait polls from the maintainer's Mac and therefore sees
  # exactly ONE Cloudflare POP; a runner resolves to another, whose edge cache
  # can still hold the previous release's copy. v0.18.3 and v0.18.5 both reddened
  # a front-door leg for that reason alone and both went green on a bare rerun.
  # So a single fetch is a SAMPLE, not a verdict, and what is pinned here is that
  # the mismatch case re-reads, that the re-read can see past the edge cache, and
  # that no OTHER verdict in the step learned to retry along with it.
  sniff="$(step_code "$job" "Every helper lib the installer needs resolves to shell")"
  rc_loop_needle='while [ "$served_version" != "$rc_version" ]'
  rc_loop_line="$(printf '%s\n' "$sniff" | line_with "$rc_loop_needle")"
  rc_deadline_line="$(printf '%s\n' "$sniff" | line_with 'rc_deadline=$(( rc_started + FD_RC_VERSION_WAIT_SECS ))')"

  if [ -n "$rc_loop_line" ] && [ -n "$rc_deadline_line" ]; then
    pass "an rc version mismatch is re-read rather than believed on the first fetch (loop line $rc_loop_line, deadline line $rc_deadline_line)"
  else
    fail "the payload-sniff step has no bounded re-read around the rc version mismatch (loop ${rc_loop_line:-missing}, deadline ${rc_deadline_line:-missing}); one sample from one Cloudflare POP would again be treated as a verdict"
  fi

  # Parsed out of the file, never restated here: a budget this test hardcodes is
  # a budget that can silently diverge from the one the job actually spends.
  rc_wait="$(printf '%s\n' "$code" | sed -n "s/^[0-9]*:[[:space:]]*FD_RC_VERSION_WAIT_SECS:[[:space:]]*'\([0-9]*\)'.*/\1/p" | head -n 1)"
  rc_poll="$(printf '%s\n' "$code" | sed -n "s/^[0-9]*:[[:space:]]*FD_RC_VERSION_POLL_SECS:[[:space:]]*'\([0-9]*\)'.*/\1/p" | head -n 1)"
  if [ -n "$rc_wait" ] && [ -n "$rc_poll" ]; then
    if [ "$rc_poll" -ge 15 ] && [ "$rc_poll" -le 60 ] && [ "$rc_wait" -gt "$rc_poll" ] && [ "$rc_wait" -le 600 ]; then
      pass "the rc re-read budget is bounded and its interval is in band: wait ${rc_wait}s, poll ${rc_poll}s"
    else
      fail "the rc re-read budget is out of band: wait ${rc_wait}s, poll ${rc_poll}s (want 15 <= poll <= 60 < wait <= 600). Too tight and it cannot outlast POP lag; too loose and it just makes a real mismatch slower."
    fi
  else
    fail "FD_RC_VERSION_WAIT_SECS / FD_RC_VERSION_POLL_SECS are not both literal integers in this job's env: an unbounded or interpolated re-read is exactly what this asserts against"
  fi

  # A re-read that hits the same cached copy for four minutes is not a retry, it
  # is a slower red. Cloudflare's cache key includes the query string, so the
  # per-attempt nonce is what forces a MISS; the no-cache headers are the second
  # half. The Access headers must survive both, or the gated RC origin answers
  # every re-read with a login page.
  cb_ok=1
  printf '%s\n' "$sniff" | grep -qF 'install.sh?cb=$(date +%s)-$rc_attempt' \
    || { cb_ok=0; fail "the re-read fetch carries no per-attempt cb= query nonce, so Cloudflare would serve it from the same POP cache the first fetch already read"; }
  rc_fetch="$(printf '%s\n' "$sniff" | grep -F 'Cache-Control: no-cache' | head -n 1)"
  if [ -z "$rc_fetch" ]; then
    cb_ok=0
    fail "the re-read fetch sends no 'Cache-Control: no-cache' header"
  else
    case "$rc_fetch" in
      *FD_HDR*'"$rc_url"'*) ;;
      *) cb_ok=0; fail "the no-cache fetch is not the cache-busted URL with the Access headers attached: $rc_fetch" ;;
    esac
  fi
  [ "$cb_ok" -eq 1 ] && pass "every re-read defeats the edge cache (cb= nonce plus no-cache headers) and keeps the Access headers"

  # Scope. The retry covers the MISMATCH on the push arm and nothing else.
  push_line="$(printf '%s\n' "$sniff" | line_with '[ "${GITHUB_EVENT_NAME:-}" = "push" ]')"
  if [ -n "$push_line" ] && [ -n "$rc_loop_line" ] && [ "$rc_loop_line" -gt "$push_line" ]; then
    pass "the re-read is inside the push arm (guard line $push_line, loop line $rc_loop_line)"
  else
    fail "the re-read loop is not inside the '\${GITHUB_EVENT_NAME:-} = push' arm (guard ${push_line:-missing}, loop ${rc_loop_line:-missing}), so it would run on events that assert no version at all"
  fi

  empty_line="$(printf '%s\n' "$sniff" | line_with 'could not parse LUCIDOS_DEFAULT_VERSION out of the served')"
  if [ -z "$empty_line" ]; then
    fail "the first-read fail-closed branch for an unparseable LUCIDOS_DEFAULT_VERSION is gone: a parser that finds nothing must never be read as 'the version is fine'"
  else
    empty_tail="$(sed -n "$((empty_line + 1)),$((empty_line + 3))p" "$WORKFLOW")"
    if printf '%s\n' "$empty_tail" | grep -qF 'exit 1' \
       && ! printf '%s\n' "$empty_tail" | grep -qE 'sleep|while '; then
      pass "an unparseable version still fails closed immediately (line $empty_line), with no re-read"
    else
      fail "the unparseable-version branch at line $empty_line no longer exits straight away: the re-read is for the MISMATCH case only, and a fail-closed parse must never wait"
    fi
  fi

  # Expiry stays fatal, and says which of the two causes it has ruled out.
  exp_rc="$(printf '%s\n' "$sniff" | grep -F 'never converged' | head -n 1)"
  if [ -z "$exp_rc" ]; then
    fail "the re-read has no expiry error separating exhausted propagation lag from a genuinely different published candidate"
  else
    exp_rc_line="${exp_rc%%:*}"
    exp_ok=1
    case "$exp_rc" in
      *'Last read: $served_version'*'Expected: $rc_version'*) ;;
      *) exp_ok=0; fail "the expiry error does not report both the last-read and the expected version" ;;
    esac
    case "$(sed -n "$((exp_rc_line + 1))p" "$WORKFLOW")" in
      *'exit 1'*) ;;
      *) exp_ok=0; fail "the expiry error at line $exp_rc_line is not immediately followed by 'exit 1': the gate must never degrade a genuine mismatch to a warning" ;;
    esac
    [ "$exp_ok" -eq 1 ] && pass "expiry still fails hard (line $exp_rc_line), naming the last-read and expected versions"
  fi

  # The export must reflect whichever copy the loop finally accepted, so it is
  # checked against the END of the push arm, not against its `while`. Compared
  # against the `while`, an export moved INSIDE the body would still pass while
  # pinning an intermediate read, which is the very shadowing this asserts on.
  rc_block="$(printf '%s\n' "$sniff" | guarded_block '[ "${GITHUB_EVENT_NAME:-}" = "push" ]; then')"
  rc_end_line="$(printf '%s\n' "$rc_block" | block_end)"
  env_line="$(printf '%s\n' "$sniff" | line_with 'echo "FD_SERVED_VERSION=$served_version" >> "$GITHUB_ENV"')"
  env_count="$(printf '%s\n' "$sniff" | grep -cF 'FD_SERVED_VERSION=$served_version')"
  if [ -n "$env_line" ] && [ -n "$rc_end_line" ] && [ "$env_line" -gt "$rc_end_line" ] && [ "$env_count" -eq 1 ]; then
    pass "FD_SERVED_VERSION is exported once, after the whole re-read block ends (line $env_line, block ends $rc_end_line)"
  else
    fail "FD_SERVED_VERSION is exported at line ${env_line:-none} ($env_count time(s)), not exactly once after the re-read block ends (${rc_end_line:-missing}): a retried version would be shadowed by the stale first read"
  fi

  # ── 11. the two install.sh fetches are pinned to ONE release ────────────────
  # Rung 1 reads the served installer once; the "Run the advertised command"
  # step re-fetches the same URL independently, seconds or minutes later.
  # Nothing tied those two reads to the same release, and in the post-publish
  # window they legitimately disagree, which makes the asset preflight between
  # them a guarantee about a release nobody then downloads. On v0.20.1 both
  # macOS legs verified 0.20.0's assets and installed 0.20.1.
  ev_block="$(printf '%s\n' "$sniff" | guarded_block 'if [ -n "${FD_EXPECT_VERSION:-}" ]; then')"
  ev_guard_line="$(printf '%s\n' "$ev_block" | line_with 'if [ -n "${FD_EXPECT_VERSION:-}" ]; then')"
  ev_loop_line="$(printf '%s\n' "$ev_block" | line_with 'while [ "$served_version" != "$FD_EXPECT_VERSION" ]')"
  if [ -n "$ev_guard_line" ] && [ -n "$ev_loop_line" ] && [ "$ev_loop_line" -gt "$ev_guard_line" ] \
     && printf '%s\n' "$code" | grep -qF 'FD_EXPECT_VERSION: ${{ inputs.expect_version'; then
    pass "rung 1 converges on the dispatched release when one is named (guard line $ev_guard_line, loop line $ev_loop_line)"
  else
    fail "the payload-sniff step has no expect_version convergence behind a non-empty guard (guard ${ev_guard_line:-missing}, loop ${ev_loop_line:-missing}), or the job does not read the input: rung 1's version and the install step's would again be free to differ"
  fi

  # Parsed out of the file for the same reason the other budgets are: one this
  # test hardcoded could silently diverge from the one the job spends.
  if [ -n "$ev_wait" ] && [ -n "$ev_poll" ]; then
    if [ "$ev_poll" -ge 15 ] && [ "$ev_poll" -le 60 ] && [ "$ev_wait" -gt "$ev_poll" ] && [ "$ev_wait" -le 1800 ]; then
      pass "the expect_version budget is bounded and its interval is in band: wait ${ev_wait}s, poll ${ev_poll}s"
    else
      fail "the expect_version budget is out of band: wait ${ev_wait}s, poll ${ev_poll}s (want 15 <= poll <= 60 < wait <= 1800). Too tight and it cannot outlast the propagation window the dispatch fires into; too loose and it just makes an undeployed release slower to report."
    fi
  else
    fail "FD_EXPECT_VERSION_WAIT_SECS / FD_EXPECT_VERSION_POLL_SECS are not both literal integers in this job's env: an unbounded or interpolated convergence is exactly what this asserts against"
  fi

  # THE NEGATIVE HALF, and the reason this section exists. The rc loop above
  # MUST cache-bust and this one must NOT, so the assertion has to be that the
  # nonce and the no-cache headers are ABSENT here. A '?cb=' query string is a
  # different Cloudflare cache key: converging on the nonced URL would say
  # nothing about the plain one, which is the only URL the install step ever
  # requests, and the drift this loop closes would quietly stay open.
  plain_ok=1
  if ! printf '%s\n' "$ev_block" | grep -qF '"$FRONT_DOOR/install.sh" -o "$tmp/install.sh"'; then
    plain_ok=0
    fail "the expect_version re-read does not fetch the plain \$FRONT_DOOR/install.sh: it must poll the exact URL the install step will request, or converging proves nothing about it"
  fi
  if printf '%s\n' "$ev_block" | grep -qF 'cb='; then
    plain_ok=0
    fail "the expect_version re-read carries a cache-busting nonce. That is correct for the rc loop and wrong here: a '?cb=' query string is a DIFFERENT Cloudflare cache key, so it would converge on a URL the install step never fetches."
  fi
  if printf '%s\n' "$ev_block" | grep -qiE 'Cache-Control|Pragma'; then
    plain_ok=0
    fail "the expect_version re-read sends no-cache request headers. Same reason as the nonce: what has to be asserted is what a stranger's unadorned curl gets from this POP."
  fi
  [ "$plain_ok" -eq 1 ] && pass "the expect_version re-read polls the PLAIN url, with no nonce and no cache headers (the rc loop deliberately does the opposite)"

  if printf '%s\n' "$ev_block" | grep -qF 'assert_shell_file "$tmp/install.sh" "$FRONT_DOOR/install.sh"'; then
    pass "each convergence re-read re-validates the whole payload, so one landing on the soft-404 page is reported as HTML"
  else
    fail "the convergence re-read does not re-run assert_shell_file, so a re-read that lands on the landing page would be swallowed as one more version mismatch"
  fi

  # Expiry stays fatal and separates the two causes it cannot tell apart from
  # the outside: nothing deployed, versus a POP still lagging.
  ev_exp="$(printf '%s\n' "$ev_block" | grep -F 'never agreed' | head -n 1)"
  if [ -z "$ev_exp" ]; then
    fail "the convergence loop has no expiry error, so an undeployed release would either hang or pass"
  else
    ev_exp_line="${ev_exp%%:*}"
    ev_ok=1
    case "$ev_exp" in
      *'TWO CAUSES'*) ;;
      *) ev_ok=0; fail "the convergence expiry does not separate 'not deployed' from 'this POP is still lagging', which are the only two readings and want different responses" ;;
    esac
    case "$ev_exp" in
      *'Last read: $served_version'*'Expected: $FD_EXPECT_VERSION'*) ;;
      *) ev_ok=0; fail "the convergence expiry does not report both the last-read and the expected version" ;;
    esac
    case "$(sed -n "$((ev_exp_line + 1))p" "$WORKFLOW")" in
      *'exit 1'*) ;;
      *) ev_ok=0; fail "the convergence expiry at line $ev_exp_line is not immediately followed by 'exit 1': it must never degrade to a warning and let the install run unpinned" ;;
    esac
    [ "$ev_ok" -eq 1 ] && pass "convergence expiry fails hard (line $ev_exp_line), naming both causes and both versions"
  fi

  # Against the END of the block again, for the reason spelled out at
  # guarded_block: an export INSIDE the body would pin an intermediate read.
  ev_end_line="$(printf '%s\n' "$ev_block" | block_end)"
  if [ -n "$env_line" ] && [ -n "$ev_end_line" ] && [ "$env_line" -gt "$ev_end_line" ]; then
    pass "FD_SERVED_VERSION is exported after the convergence block ends too (line $env_line, block ends $ev_end_line)"
  else
    fail "FD_SERVED_VERSION is exported at line ${env_line:-none}, not after the convergence block ends (${ev_end_line:-missing}): the preflight would probe the version of an earlier read rather than the converged one"
  fi

  # A malformed input can never equal the served version, so it would cost the
  # whole budget and then red for a reason that is not the origin's. Refused up
  # front instead, in the same step and before the same first fetch as the
  # hostile-origin allowlist.
  validate="$(step_code "$job" "Resolve and validate the origin under test")"
  shape_line="$(printf '%s\n' "$validate" | line_with 'refusing expect_version')"
  if [ -n "$shape_line" ] && [ -n "$curl_line" ] && [ "$shape_line" -lt "$curl_line" ]; then
    pass "a malformed expect_version is refused in the validate step (line $shape_line), before the first fetch (line $curl_line)"
  else
    fail "expect_version is not shape-checked before the first fetch (refusal ${shape_line:-missing}, first fetch ${curl_line:-none}): a v<ver> tag would burn the entire convergence budget before failing"
  fi

  # The residual drift, named as itself. The rename must not have EATEN the
  # plain verdict: a download failure whose URL names the verified version is
  # still a download failure.
  if printf '%s\n' "$verify" | grep -qF 'FRONT-DOOR VERSION DRIFT' \
     && printf '%s\n' "$verify" | grep -qF '[ "$failed_version" != "$FD_SERVED_VERSION" ]' \
     && printf '%s\n' "$verify" | grep -qF 'This is a DOWNLOAD failure, not a gateway health failure'; then
    pass "a download failure naming a different version than rung 1 verified is reported as drift, and the plain verdict still exists for the rest"
  else
    fail "assert_no_download_failure does not tell version drift from a download failure (or the rename replaced the plain verdict instead of preceding it), so the v0.20.1 shape would again point at release-tarballs.yml over an asset this run never verified"
  fi
done

# ── the one asymmetry between the jobs, asserted in both directions ───────────
# On Linux the container has no service manager, so install.sh degrades to a
# foreground launch and an exited installer can only mean failure: the fast-fail
# is CORRECT there. On macOS launchd exists, install.sh exits 0 with the gateway
# detached, and the same check would abort the poll on the healthy path. Each
# job must therefore carry the opposite of the other.
echo
echo "the macOS launch-shape asymmetry"
linux_verify="$(step_code front-door "Verify the front door installed a working Lucidos")"
macos_verify="$(step_code front-door-macos "Verify the front door installed a working Lucidos")"
# The literal text searched for in both jobs, held once so the two searches
# cannot drift apart. Single-quoted deliberately: it is a fixed string to find
# in the workflow, not something to expand here.
FASTFAIL='kill -0 "$INSTALL_PID"'
if printf '%s\n' "$linux_verify" | grep -qF -- "$FASTFAIL"; then
  pass "front-door keeps its installer-exited fast-fail (a foreground launch that exits has failed)"
else
  fail "front-door lost its '$FASTFAIL' fast-fail, so a dead installer would poll to the deadline"
fi
if printf '%s\n' "$macos_verify" | grep -qF -- "$FASTFAIL"; then
  fail "front-door-macos has an installer-exited fast-fail: on launchd the installer exits 0 BY DESIGN, so this aborts the healthy path"
else
  pass "front-door-macos has no installer-exited fast-fail (an exited installer is never by itself a verdict there)"
fi

# ── the preflight's URL construction still matches the tree ───────────────────
# The origin serves a copy of these libs, published from this tree at release
# time, so tree-side drift is the likeliest way the preflight starts probing a
# URL no installer would request. The preflight derives the base URL and pins
# the stem; both assumptions are re-checked here against the real libs.
echo
echo "URL fidelity against the tree's own libs"
base_fmt="$(sed -n '/^install_default_base_url()/,/^}/p' "$PROJECT_DIR/scripts/lib/install_common.sh" \
            | sed -n "s/.*printf '\([^']*\)'.*/\1/p" | head -n 1)"
case "$base_fmt" in
  https://*%s)
    trimmed="${base_fmt%'%s'}"
    case "$trimmed" in
      *%*) fail "install_default_base_url's format carries more than one printf directive ('$base_fmt'); the preflight can only substitute the version" ;;
      *)   pass "install_default_base_url is derivable by the preflight ('$base_fmt')" ;;
    esac
    ;;
  *)
    fail "install_default_base_url prints '$base_fmt', which the preflight's 'https://...%s' derivation cannot handle"
    ;;
esac

stem_fmt="$(sed -n '/^headless_tarball_stem()/,/^}/p' "$PROJECT_DIR/scripts/lib/headless_tarball.sh" \
            | sed -n "s/.*printf '\([^']*\)'.*/\1/p" | head -n 1)"
if [ "$stem_fmt" = 'lucidos-%s-%s' ]; then
  pass "headless_tarball_stem still builds 'lucidos-%s-%s', which is what the preflight constructs"
else
  fail "headless_tarball_stem builds '$stem_fmt'; the preflight pins 'lucidos-%s-%s' and would probe the wrong asset"
fi

# ── the download-failure pattern still matches what install.sh prints ─────────
# Matching the installer's own wording is the whole mechanism, and that wording
# lives in another file. A reworded die() here would silently disarm the check.
echo
echo "the download-failure pattern matches install.sh"
for frag in 'Download failed:' 'could NOT fetch its checksum sidecar'; do
  if grep -qF -- "$frag" "$PROJECT_DIR/install.sh"; then
    pass "install.sh still prints: $frag"
  else
    fail "install.sh no longer prints '$frag', so assert_no_download_failure would never fire on it"
  fi
done

echo
echo "front-door gate: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
