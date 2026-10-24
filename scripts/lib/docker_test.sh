#!/bin/bash
# Tests for scripts/lib/docker.sh: the shared Docker-daemon probe, its
# classification table, and the report a launcher prints when the daemon is down.
#
# HERMETIC BY CONSTRUCTION, and that is the load-bearing property here, not a
# nicety. This library's whole job is to run `docker` and to run `open -a Docker`,
# and a test that reached either would start Docker Desktop on the developer's
# machine or mutate a real daemon. The three host-touching functions
# (`_docker_cli_present`, `_docker_version_probe`, `_docker_version_stderr`) are
# the ONLY seams, they are stubbed for the whole file, and `assert_no_host_calls`
# below fails the suite if a stub was bypassed. Same posture as ports_test.sh's
# `kill` shim and webkit_reaper_test.sh's `ps` feed, for the same reason: a lib
# test that can reach the host is one refactor away from being an incident
# (ADR 0025; the 2026-07-28 and 2026-08-03 kills).
#
# `ensure_docker_daemon` is deliberately NOT exercised end to end: its macOS arm
# calls `open -a Docker` and its failure arm calls `exit`. Its decision content
# lives in `docker_daemon_state` + `docker_down_report`, both covered here, and
# in `_confirm_default_yes`, whose non-interactive arm is covered directly.
#
# Run: ./scripts/lib/docker_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/docker.sh
source "$SCRIPT_DIR/docker.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

assert_eq() { # <expected> <actual> <label>
    if [ "$1" = "$2" ]; then pass "$3"; else fail "$3 (expected '$1', got '$2')"; fi
}
assert_contains() { # <haystack> <needle> <label>
    case "$1" in
        *"$2"*) pass "$3" ;;
        *) fail "$3 (missing '$2' in: $1)" ;;
    esac
}
assert_not_contains() { # <haystack> <needle> <label>
    case "$1" in
        *"$2"*) fail "$3 (unexpectedly found '$2')" ;;
        *) pass "$3" ;;
    esac
}

# ── Host seams, stubbed for the whole file ──────────────────────────────────
# Set by each scenario; read by the stubs. HOST_CALLS counts any attempt to
# reach the real host, which must stay 0.
STUB_CLI_PRESENT=0
STUB_PROBE_STATUS=0
STUB_STDERR=""
STUB_OS="Darwin"
HOST_CALLS=0

_docker_cli_present() { return "$STUB_CLI_PRESENT"; }
_docker_version_probe() { return "$STUB_PROBE_STATUS"; }
_docker_version_stderr() { printf '%s\n' "$STUB_STDERR"; }
# The OS is stubbed for the same reason as the rest: the report's remedies differ
# per platform, and a suite that could only ever see its own host would leave the
# Linux arms untested. That is not hypothetical, it is how they shipped wrong.
_docker_host_os() { printf '%s\n' "$STUB_OS"; }

# A `docker` on PATH would mean a stub was bypassed somewhere. Shadow the binary
# itself so the bypass is caught rather than executed, and likewise `open`, which
# is the one call in this library that would visibly hijack the developer's
# machine.
docker() { HOST_CALLS=$((HOST_CALLS + 1)); return 125; }
open() { HOST_CALLS=$((HOST_CALLS + 1)); return 125; }
sleep() { HOST_CALLS=$((HOST_CALLS + 1)); return 0; }

assert_no_host_calls() {
    assert_eq 0 "$HOST_CALLS" "no test reached the real docker/open/sleep"
}

echo "classify_docker_probe (pure)"
# A missing CLI wins over any probe status: waiting cannot install docker, so
# this is the arm the gateway calls Terminal.
assert_eq missing "$(classify_docker_probe 1 0)" "no CLI is 'missing' even if a probe somehow passed"
assert_eq missing "$(classify_docker_probe 127 1)" "any non-zero CLI status is 'missing'"
assert_eq ready "$(classify_docker_probe 0 0)" "CLI present and probe exit 0 is 'ready'"
assert_eq unreachable "$(classify_docker_probe 0 1)" "CLI present and probe exit 1 is 'unreachable'"
# Docker's CLI does not promise exit 1 specifically; anything non-zero from the
# server half means the daemon did not answer.
assert_eq unreachable "$(classify_docker_probe 0 125)" "any non-zero probe status is 'unreachable'"

echo ""
echo "docker_daemon_state (seams -> state)"
STUB_CLI_PRESENT=0; STUB_PROBE_STATUS=0
assert_eq ready "$(docker_daemon_state)" "daemon answering reports ready"
STUB_CLI_PRESENT=0; STUB_PROBE_STATUS=1
assert_eq unreachable "$(docker_daemon_state)" "daemon not answering reports unreachable"
STUB_CLI_PRESENT=1; STUB_PROBE_STATUS=0
assert_eq missing "$(docker_daemon_state)" "absent CLI reports missing without probing"

echo ""
echo "docker_probe_detail (report-only, never classified on)"
STUB_STDERR="error during connect: Get \"http://x/version\": dial unix /var/run/docker.sock: connect: no such file or directory"
assert_contains "$(docker_probe_detail)" "dial unix" "quotes the daemon's own words"
assert_not_contains "$(docker_probe_detail)" "error during connect:" "strips the CLI's own prefix"
STUB_STDERR=""
assert_eq "" "$(docker_probe_detail)" "silent daemon yields no detail line"
STUB_STDERR="$(printf '\n\npermission denied while trying to connect\nsecond line')"
assert_eq "permission denied while trying to connect" "$(docker_probe_detail)" "takes the first non-blank line only"
STUB_STDERR="$(printf 'x%.0s' $(seq 1 300))"
DETAIL="$(docker_probe_detail)"
assert_eq 120 "${#DETAIL}" "a very long complaint is truncated to 120 chars"
assert_contains "$DETAIL" "..." "truncation is visible"

echo ""
echo "docker_down_report (the block the user actually reads)"
SCRIPT_NAME="web-dev.sh"
WORKSPACE="dev"
BUILD="1"
RELEASE=""
STUB_STDERR="Cannot connect to the Docker daemon"
REPORT="$(docker_down_report unreachable 2>&1)"
assert_contains "$REPORT" "Docker is not running" "names the condition"
assert_contains "$REPORT" "PostgreSQL" "says why it blocks everything"
assert_contains "$REPORT" "Cannot connect to the Docker daemon" "quotes the daemon"
assert_contains "$REPORT" "./scripts/web-dev.sh -w dev -b" "reproduces the caller's own command"
# The whole point of the block: it cannot be mistaken for ordinary launch chatter.
assert_contains "$REPORT" "────" "is visually delimited"

# Written to stderr so a caller piping stdout somewhere still sees it.
assert_eq "" "$(docker_down_report unreachable 2>/dev/null)" "the report goes to stderr, not stdout"

MISSING_REPORT="$(docker_down_report missing 2>&1)"
assert_contains "$MISSING_REPORT" "Docker is not installed" "the missing arm has its own headline"
assert_contains "$MISSING_REPORT" "brew install --cask docker" "the missing arm offers the install, not a start"
assert_not_contains "$MISSING_REPORT" "Docker said:" "no daemon quote when there is no daemon to quote"

RELEASE="1"
assert_contains "$(docker_down_report unreachable 2>&1)" "-w dev -b -r" "carries every flag the caller passed"
RELEASE=""

# An early failure (before -w is parsed) must still produce a usable command.
WORKSPACE=""; BUILD=""
assert_contains "$(docker_down_report unreachable 2>&1)" "./scripts/web-dev.sh" "degrades to the bare script when no workspace is known"
WORKSPACE="dev"; BUILD="1"

# The re-run line exists to be pasted, so a workspace path that would split on
# paste has to come back quoted.
WORKSPACE="/tmp/my workspaces/dev"
assert_contains "$(docker_down_report unreachable 2>&1)" '-w "/tmp/my workspaces/dev"' "quotes a workspace path containing a space"
WORKSPACE="dev"
assert_not_contains "$(docker_down_report unreachable 2>&1)" '-w "dev"' "and leaves an ordinary one unquoted"

echo ""
echo "docker_down_report on a non-Darwin host"
# These arms were unreachable from a macOS dev machine before `_docker_host_os`
# became a seam, and both of them shipped wrong: a Homebrew command Linux cannot
# run, and a service-start prescribed for every unreachable daemon including a
# permission error where the service is already up.
STUB_OS="Linux"

LINUX_MISSING="$(docker_down_report missing 2>&1)"
assert_not_contains "$LINUX_MISSING" "brew" "never prescribes Homebrew on Linux"
assert_contains "$LINUX_MISSING" "docs.docker.com/engine/install" "points Linux at the real install docs"

STUB_STDERR="permission denied while trying to connect to the Docker daemon socket"
LINUX_DOWN="$(docker_down_report unreachable 2>&1)"
# The state is classified WITHOUT deciding why (exit status only), so the report
# must not assert one cause. Both real candidates are offered and the quoted
# error is what discriminates, matching install.sh's own Linux diagnosis.
assert_contains "$LINUX_DOWN" "permission denied" "quotes the error that says which cause it is"
assert_contains "$LINUX_DOWN" "sudo systemctl start docker" "offers the stopped-service case"
assert_contains "$LINUX_DOWN" "'docker' group" "and the permission case install.sh also names"
assert_not_contains "$LINUX_DOWN" "Start Docker:" "does not present one guess as THE fix"
assert_not_contains "$LINUX_DOWN" "open -a Docker" "never prescribes a macOS command on Linux"

STUB_OS="Darwin"
STUB_STDERR="Cannot connect to the Docker daemon"

echo ""
echo "_confirm_default_yes"
# The invariant that keeps a restart-API-spawned launch from hanging forever on a
# keystroke nobody is there to press.
assert_eq 1 "$(_confirm_default_yes "start?" </dev/null >/dev/null 2>&1; echo $?)" "non-interactive stdin declines instead of blocking"

echo ""
echo "ensure_docker_daemon, non-interactively"
# Run in a subshell so its `exit 1` ends the subshell rather than the suite, and
# with stdin closed so the macOS arm's prompt is unreachable. The stubs make the
# daemon unreachable; nothing here may reach `open`.
STUB_CLI_PRESENT=0; STUB_PROBE_STATUS=1; STUB_STDERR="Cannot connect to the Docker daemon"
ENSURE_OUT="$( (ensure_docker_daemon) </dev/null 2>&1 )"
ENSURE_RC=$?
assert_eq 1 "$ENSURE_RC" "a down daemon fails the launch"
assert_contains "$ENSURE_OUT" "Docker is not running" "and prints the report"
assert_not_contains "$ENSURE_OUT" "Start Docker Desktop?" "without asking a question nobody can answer"
assert_not_contains "$ENSURE_OUT" "daemon isn't running." "and without the prompt's orphaned lead-in"

STUB_PROBE_STATUS=0
( ensure_docker_daemon ) </dev/null >/dev/null 2>&1
assert_eq 0 $? "a ready daemon returns cleanly"

echo ""
assert_no_host_calls

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
