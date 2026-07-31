#!/bin/bash
# Tests for scripts/lib/release_rc_front_door.sh — the ordering guard that makes
# the RC front-door gate meaningful.
#
# The property under test is narrow and load-bearing: rc/<version> must not be
# pushed until https://rc.lucidos.dev serves THIS candidate's installer. CI's
# payload-mode rung 1 parses LUCIDOS_DEFAULT_VERSION out of that origin and fails
# on a mismatch, so pushing first produces a red that means nothing.
#
# What this pins:
#   1. the version sniff accepts ONLY an installer baking the expected version —
#      and fails closed on the soft-404 HTML page, which arrives at status 200
#      and is the exact failure that started this work;
#   2. the wait loop returns as soon as the origin flips, and times out rather
#      than hanging a release;
#   3. arming is idempotent — an origin already serving this version costs no
#      deploy, which is what makes `--push-rc` retries cheap;
#   4. every failure path is NON-FATAL and returns non-zero, so a slow publisher
#      can never abort a release;
#   5. the wiring in release.sh: the arm call precedes the branch push on every
#      path that pushes an RC.
#
# Hermetic and offline: the origin is a `file://` tree, so no network, no
# Cloudflare, no `lucidos`. Curl reads local files, which exercises the real
# fetch + sniff path rather than a stub.
#
# Run: ./scripts/lib/release_rc_front_door_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/release_rc_front_door.sh
source "$SCRIPT_DIR/release_rc_front_door.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

ORIGIN_DIR="$(mktemp -d)"
trap 'rm -rf "$ORIGIN_DIR"' EXIT
ORIGIN="file://$ORIGIN_DIR"

# An installer shaped like the real one, for a given baked version.
write_installer() { # <version>
  cat > "$ORIGIN_DIR/install.sh" <<EOF
#!/bin/sh
set -eu
LUCIDOS_DEFAULT_VERSION="$1"
LUCIDOS_INSTALL_URL="\${LUCIDOS_INSTALL_URL:-https://rc.lucidos.dev/install.sh}"
echo installing
EOF
}

# The soft-404: Cloudflare Pages answers a missing path with the landing page at
# status 200, so `curl -f` succeeds and only the BODY reveals the truth.
write_soft_404() {
  cat > "$ORIGIN_DIR/install.sh" <<'EOF'
<!DOCTYPE html>
<html><head><title>Lucidos</title></head>
<body>If you can describe it, it exists.</body></html>
EOF
}

# The same, but with leading blank lines and indentation — a prettified SPA
# fallback must not sneak past a naive first-byte check.
write_soft_404_indented() {
  printf '\n\n   <html>\n     <body>Lucidos</body>\n   </html>\n' > "$ORIGIN_DIR/install.sh"
}

echo "== 1. the version sniff =="

write_installer "0.78.0"
if rc_front_door_serves_version "0.78.0" "$ORIGIN"; then
  pass "accepts an installer baking the expected version"
else
  fail "rejected an installer that bakes 0.78.0"
fi

if rc_front_door_serves_version "0.77.0" "$ORIGIN"; then
  fail "accepted 0.77.0 while the origin serves 0.78.0 — the previous-RC hole"
else
  pass "rejects a DIFFERENT version (the previous candidate cannot pass the gate)"
fi

write_soft_404
if rc_front_door_serves_version "0.78.0" "$ORIGIN"; then
  fail "accepted the soft-404 landing page as an installer"
else
  pass "rejects a soft-404 HTML payload served at 200"
fi

write_soft_404_indented
if rc_front_door_serves_version "0.78.0" "$ORIGIN"; then
  fail "accepted indented HTML as an installer"
else
  pass "rejects HTML behind blank lines and indentation"
fi

: > "$ORIGIN_DIR/install.sh"
if rc_front_door_serves_version "0.78.0" "$ORIGIN"; then
  fail "accepted an empty body"
else
  pass "rejects an empty body"
fi

# A shell script that is real but carries no version line: unparsable, so the
# check must fail CLOSED rather than treat "not found" as "matches".
printf '#!/bin/sh\necho hi\n' > "$ORIGIN_DIR/install.sh"
if rc_front_door_serves_version "0.78.0" "$ORIGIN"; then
  fail "passed an installer with no LUCIDOS_DEFAULT_VERSION"
else
  pass "fails closed when the version cannot be parsed"
fi

rm -f "$ORIGIN_DIR/install.sh"
if rc_front_door_serves_version "0.78.0" "$ORIGIN"; then
  fail "passed with nothing at the origin at all"
else
  pass "fails when the origin serves nothing (hard 404 / fetch error)"
fi

echo "== 2. the wait loop =="

write_installer "0.78.0"
start=$(date +%s)
if rc_front_door_wait "0.78.0" 30 "$ORIGIN" >/dev/null; then
  elapsed=$(( $(date +%s) - start ))
  if (( elapsed <= 2 )); then
    pass "returns immediately when the origin already serves the version (${elapsed}s)"
  else
    fail "took ${elapsed}s to notice an origin that was already correct"
  fi
else
  fail "wait failed against a correct origin"
fi

write_installer "0.77.0"
start=$(date +%s)
if rc_front_door_wait "0.78.0" 10 "$ORIGIN" >/dev/null 2>&1; then
  fail "wait returned success while the origin served the WRONG version"
else
  elapsed=$(( $(date +%s) - start ))
  if (( elapsed >= 10 && elapsed <= 20 )); then
    pass "times out (${elapsed}s) instead of hanging, and reports failure"
  else
    fail "timeout took ${elapsed}s, expected ~10s"
  fi
fi

echo "== 3. arming is idempotent and never fatal =="

# Already-correct origin: must short-circuit WITHOUT needing `lucidos` at all,
# which is what makes a --push-rc retry cheap.
write_installer "0.78.0"
out="$(PATH=/usr/bin:/bin rc_front_door_arm "$PROJECT_DIR" "0.78.0" 5 "$ORIGIN" 2>&1)"
rc=$?
if (( rc == 0 )) && [[ "$out" == *"already serves"* ]]; then
  pass "an origin already serving the version needs no publish"
else
  fail "expected a no-op short-circuit, got rc=$rc: $out"
fi

# No `lucidos` on PATH and an origin that does NOT serve the version: cannot
# request a publish, must degrade rather than die.
write_installer "0.77.0"
out="$(PATH=/usr/bin:/bin rc_front_door_arm "$PROJECT_DIR" "0.78.0" 5 "$ORIGIN" 2>&1)"
rc=$?
if (( rc != 0 )); then
  pass "returns non-zero when it cannot arm the front door"
else
  fail "reported success with no way to publish"
fi
if [[ "$out" == *"not on PATH"* ]]; then
  pass "explains that 'lucidos' is unavailable rather than failing silently"
else
  fail "unhelpful message when lucidos is missing: $out"
fi

# The library must never call fail()/exit — a release cannot die here. If any
# function exited, the subshell above would not have returned to us; assert the
# stronger property that sourcing defines no exit-on-error behaviour.
if ( set +e; PATH=/usr/bin:/bin rc_front_door_request_publish "/nonexistent" "0.78.0" >/dev/null 2>&1; true ); then
  pass "a bad RC tree returns rather than aborting the shell"
else
  fail "rc_front_door_request_publish aborted its shell"
fi

echo "== 3b. Cloudflare Access headers (the gated RC origin) =="

# The RC origin is behind Access, so every fetch must carry a service token.
# These run against the file:// origin — curl ignores the headers there, which
# is exactly the point: what is under test is that BUILDING the header array
# does not blow up, on either branch. macOS ships bash 3.2, where expanding an
# empty array under `set -u` is an unbound-variable ABORT, so the credential-less
# path died instead of returning 1 (observed 2026-07-30). Both outcomes are
# non-zero, so the bug hid inside a passing test until stderr was read.

write_installer "9.9.9"

# Each case runs in its own subshell so the credentials it sets cannot leak into
# the next one — the credential-less cases only mean anything if nothing earlier
# exported a token. That deliberate scoping is what SC2030/SC2031 flag, hence the
# disables below; they are the mechanism under test, not an accident. Written as
# `if ( … ); then` rather than `( … ) && pass || fail`, which would run BOTH arms
# if `pass` ever returned non-zero.

# shellcheck disable=SC2030 # scoping the credentials to this case is the point
if (
  unset RC_ACCESS_CLIENT_ID RC_ACCESS_CLIENT_SECRET
  export RC_ACCESS_ENV_FILE=/nonexistent/rc-access.env
  err="$(rc_front_door_serves_version "9.9.9" "$ORIGIN" 2>&1 >/dev/null)"
  [ -z "$err" ]
); then
  pass "no credentials: builds an empty header array without an unbound-variable abort"
else
  fail "no credentials: wrote to stderr (bash 3.2 empty-array expansion under set -u)"
fi

# shellcheck disable=SC2030,SC2031 # ditto: per-case credentials, deliberately not shared
if (
  export RC_ACCESS_CLIENT_ID=test-id RC_ACCESS_CLIENT_SECRET=test-secret
  err="$(rc_front_door_serves_version "9.9.9" "$ORIGIN" 2>&1 >/dev/null)"
  [ -z "$err" ]
); then
  pass "with credentials: builds the header array cleanly"
else
  fail "with credentials: wrote to stderr"
fi

# shellcheck disable=SC2031 # ditto: per-case credentials, deliberately not shared
if (
  export RC_ACCESS_CLIENT_ID=test-id RC_ACCESS_CLIENT_SECRET=test-secret
  rc_front_door_serves_version "9.9.9" "$ORIGIN"
); then
  pass "with credentials: still parses the served version correctly"
else
  fail "with credentials: header array broke the fetch"
fi

# The env file is the operator's convenience path (release.sh runs on the Mac,
# not in CI). An unreadable one must be inert, not fatal.
# shellcheck disable=SC2031 # ditto: per-case credentials, deliberately not shared
if (
  unset RC_ACCESS_CLIENT_ID RC_ACCESS_CLIENT_SECRET
  export RC_ACCESS_ENV_FILE=/nonexistent/rc-access.env
  rc_access_headers >/dev/null 2>&1
); then
  fail "a missing env file reported credentials as available"
else
  pass "a missing env file yields no headers rather than erroring"
fi

echo "== 4. release.sh wiring: publish /rc BEFORE pushing rc/ =="

RELEASE_SH="$PROJECT_DIR/scripts/release.sh"

if grep -q 'release_rc_front_door.sh' "$RELEASE_SH"; then
  pass "release.sh sources release_rc_front_door.sh"
else
  fail "release.sh does not source release_rc_front_door.sh"
fi

# The ordering property itself: inside push_release_candidate, the arm call must
# appear BEFORE the git push of the rc branch. This is the whole point of the
# file, so it is asserted structurally rather than by comment.
fn="$(awk '/^push_release_candidate\(\) \{/,/^\}/' "$RELEASE_SH")"
arm_line="$(printf '%s' "$fn" | grep -n 'rc_front_door_arm' | head -1 | cut -d: -f1)"
push_line="$(printf '%s' "$fn" | grep -n 'push --force .*RC_BRANCH' | head -1 | cut -d: -f1)"
if [[ -n "$arm_line" && -n "$push_line" ]]; then
  if (( arm_line < push_line )); then
    pass "the RC front door is armed before the branch push (line $arm_line < $push_line)"
  else
    fail "arm at $arm_line comes AFTER the push at $push_line — the gate would test the previous RC"
  fi
else
  fail "could not locate both the arm call ($arm_line) and the branch push ($push_line)"
fi

echo ""
echo "  $PASS passed, $FAIL failed"
[[ $FAIL -eq 0 ]]
