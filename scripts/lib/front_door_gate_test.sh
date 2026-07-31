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
  if [ -n "$wait_secs" ] && [ -n "$health_secs" ] && [ -n "$timeout_min" ]; then
    needed=$((wait_secs + health_secs + SLACK_SECS))
    ceiling=$((timeout_min * 60))
    if [ "$needed" -le "$ceiling" ]; then
      pass "the budgets fit: ${wait_secs}s preflight + ${health_secs}s health + ${SLACK_SECS}s slack = ${needed}s <= ${ceiling}s (timeout-minutes: $timeout_min)"
    else
      fail "the budgets do NOT fit: ${needed}s needed but timeout-minutes is $timeout_min (${ceiling}s). A job timeout is a hard kill with no diagnosis."
    fi
  else
    fail "cannot do the timeout arithmetic: one of FD_ASSET_WAIT_SECS / GW_HEALTH_TIMEOUT_SECS / timeout-minutes is not a literal"
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
