#!/bin/bash
# Tests for scripts/lib/preflight.sh: check_prereqs and its build-only mode.
#
# Hermetic. The real function installs Homebrew packages and can launch Docker
# Desktop, so nothing here calls the real thing: `_check_or_install` and
# `ensure_docker_daemon` are replaced with recorders after sourcing, and the
# test reads what the function ASKED FOR rather than what a host happens to
# have. That also makes the outcome independent of whether Docker is running on
# the machine running the suite.
#
# What it pins: `check_prereqs build-only` skips the Docker DAEMON check and
# keeps every tool check, and a bare `check_prereqs` still reaches the daemon.
# A background engine rebuild (web-dev.sh --engine-build) compiles a binary and
# opens no database, so a stopped Docker Desktop must not fail it. The full
# launch still must, because setup_postgres is right behind it.
# See docs/plans/2026-09-18-engine-build-only-needs-no-ports.md.
#
# Run: ./scripts/lib/preflight_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/preflight.sh
source "$SCRIPT_DIR/preflight.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# ── recorders, installed over the real effectful helpers ────────────────
# Sourced first so these definitions win. `check_prereqs` calls both by name.

DAEMON_CHECKS=0
TOOLS_ASKED=""

# shellcheck disable=SC2329 # a seam: check_prereqs calls this, not this file
ensure_docker_daemon() {
    DAEMON_CHECKS=$((DAEMON_CHECKS + 1))
}

# shellcheck disable=SC2329 # a seam: check_prereqs calls this, not this file
_check_or_install() {
    TOOLS_ASKED="$TOOLS_ASKED $1"
}

run_prereqs() { # <mode-or-empty>
    DAEMON_CHECKS=0
    TOOLS_ASKED=""
    if [ -n "$1" ]; then check_prereqs "$1"; else check_prereqs; fi
}

expect_daemon() { # <expected-count> <what-was-being-checked>
    if [ "$DAEMON_CHECKS" -eq "$1" ]; then
        pass "$2"
    else
        fail "$2 (ensure_docker_daemon ran $DAEMON_CHECKS times, wanted $1)"
    fi
}

expect_tool() { # <tool> <what-was-being-checked>
    case " $TOOLS_ASKED " in
        *" $1 "*) pass "$2" ;;
        *) fail "$2 (never asked for '$1'; asked for:$TOOLS_ASKED)" ;;
    esac
}

echo "check_prereqs build-only"

run_prereqs build-only
expect_daemon 0 "the Docker daemon check is skipped"
# Every tool a compile needs is still demanded. Only reachable on Darwin: the
# non-Darwin arm warns rather than calling _check_or_install, and returns early.
if [[ "$OSTYPE" == "darwin"* ]]; then
    expect_tool cargo "cargo is still required"
    expect_tool node  "node is still required"
    expect_tool cmake "cmake is still required"
    expect_tool docker "the docker CLI check is untouched (only the DAEMON is skipped)"
else
    pass "tool checks skipped on this host (non-Darwin arm warns instead)"
fi

echo "check_prereqs, full launch"

run_prereqs ""
expect_daemon 1 "a bare call still reaches the Docker daemon check"

echo "an unrecognised mode fails closed"

# A typo must take the full path, never silently skip the daemon check that
# stands between a launch and an opaque `docker run failed:`.
run_prereqs build_only
expect_daemon 1 "an underscore typo still reaches the Docker daemon check"
run_prereqs BUILD-ONLY
expect_daemon 1 "a differently-cased mode still reaches the Docker daemon check"

echo ""
echo "preflight_test.sh: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
