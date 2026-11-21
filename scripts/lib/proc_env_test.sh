#!/bin/bash
# Tests for scripts/lib/proc_env.sh, the `ps -E` value parser.
#
# Run: ./scripts/lib/proc_env_test.sh   (no harness; direct, like
# host_memory_guard_test.sh)
#
# `proc_env_value` runs no command: every fixture for it is a literal line of
# the shape `ps -E -p <pid> -o command=` prints. `proc_env_has_entry` does read
# a process, so its case spawns two sleepers and reads only those. Nothing here
# signals a pid it did not spawn.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=proc_env.sh
source "$SCRIPT_DIR/proc_env.sh"

PASS=0
FAIL=0
assert_eq() {
    if [ "$2" = "$3" ]; then
        echo "  ok:   $1"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $1 (want '$3', got '$2')"
        FAIL=$((FAIL + 1))
    fi
}

# A packaged engine's real shape: argv, then the pairs, with the workspace path
# holding a space and LUCIDOS_WORKSPACE_ID following it.
PACKAGED="/Applications/Lucidos.app/Contents/MacOS/lucidos-engine \
LUCIDOS_WORKSPACE=/home/u/Library/Application Support/com.lucidos.app/workspaces/packaged \
LUCIDOS_WORKSPACE_ID=packaged LUCIDOS_GATEWAY_PORT=5252 LUCIDOS_API_PORT=59704 PATH=/usr/bin:/bin"

echo "proc_env: a value is read out of the line"
assert_eq "the workspace id" "$(proc_env_value "$PACKAGED" LUCIDOS_WORKSPACE_ID)" "packaged"
assert_eq "the gateway port" "$(proc_env_value "$PACKAGED" LUCIDOS_GATEWAY_PORT)" "5252"
assert_eq "the last pair on the line" "$(proc_env_value "$PACKAGED" PATH)" "/usr/bin:/bin"

echo "proc_env: a value holding a space runs to the next NAME= token"
assert_eq "the whole packaged workspace path" \
    "$(proc_env_value "$PACKAGED" LUCIDOS_WORKSPACE)" \
    "/home/u/Library/Application Support/com.lucidos.app/workspaces/packaged"

echo "proc_env: an absent variable prints nothing"
assert_eq "no such variable" "$(proc_env_value "$PACKAGED" LUCIDOS_NOT_SET)" ""
assert_eq "an empty line" "$(proc_env_value "" LUCIDOS_WORKSPACE)" ""

# The leading-space match is what separates these two. Without it, asking for
# GATEWAY_PORT would answer with LUCIDOS_GATEWAY_PORT's value.
echo "proc_env: a name that is the tail of another name is not a match"
SUFFIX="/opt/lucidos/bin/lucidos-engine LUCIDOS_GATEWAY_PORT=5252 PATH=/bin"
assert_eq "GATEWAY_PORT is not LUCIDOS_GATEWAY_PORT" \
    "$(proc_env_value "$SUFFIX" GATEWAY_PORT)" ""
assert_eq "the full name still reads" \
    "$(proc_env_value "$SUFFIX" LUCIDOS_GATEWAY_PORT)" "5252"

# argv comes first on the line, and it can mention anything. A workspace path in
# a command-line argument is not that process's environment.
echo "proc_env: a mention in argv is not a value"
ARGV="/opt/lucidos/bin/lucidos-engine --note LUCIDOS_WORKSPACE=/tmp/decoy LUCIDOS_WORKSPACE=/real/ws PATH=/bin"
assert_eq "the first match wins, which is the leftmost token" \
    "$(proc_env_value "$ARGV" LUCIDOS_WORKSPACE)" "/tmp/decoy"

# proc_env_has_entry is what the e2e sweeps kill on (ADR 0251), so the decoy
# above must NOT fool it. A process that only MENTIONS the entry in argv, like a
# coding agent quoting this file, does not hold it.
#
# macOS `ps -E` hides the environment of Apple platform binaries (/bin/sleep,
# /bin/sh), so there the sleeper is node. Playwright browsers are not platform
# binaries. Without this, the argv case would pass on an unreadable environment.
echo "proc_env: proc_env_has_entry reads the environment, never argv"
if [ -d /proc/$$ ]; then
    sleeper_argv=(sh -c 'while :; do sleep 1; done')
else
    sleeper_argv=("$(command -v node 2>/dev/null)" -e 'setTimeout(() => {}, 600000)')
fi
if [ -z "${sleeper_argv[0]}" ]; then
    echo "  FAIL: no non-platform sleeper binary (node) to test the reader with"
    FAIL=$((FAIL + 1))
else
    env -i PATH=/usr/bin:/bin LUCIDOS_E2E_RUN_ID=env-run "${sleeper_argv[@]}" >/dev/null 2>&1 &
    in_env=$!
    env -i PATH=/usr/bin:/bin "${sleeper_argv[@]}" "LUCIDOS_E2E_RUN_ID=argv-run" >/dev/null 2>&1 &
    in_argv=$!
    trap 'kill -KILL "$in_env" "$in_argv" 2>/dev/null' EXIT
    sleep 0.5   # let `env` exec into the sleeper, so ps reads the final image
    answer() { if proc_env_has_entry "$1" "$2"; then echo yes; else echo no; fi; }
    assert_eq "an entry in the environment is found" \
        "$(answer "$in_env" LUCIDOS_E2E_RUN_ID=env-run)" yes
    assert_eq "a PREFIX of the value is not the entry" \
        "$(answer "$in_env" LUCIDOS_E2E_RUN_ID=env)" no
    assert_eq "the argv-only sleeper's environment is readable" \
        "$(answer "$in_argv" PATH=/usr/bin:/bin)" yes
    assert_eq "an entry that is only in argv is not found" \
        "$(answer "$in_argv" LUCIDOS_E2E_RUN_ID=argv-run)" no
    assert_eq "an empty entry is never found" "$(answer "$in_env" "")" no
    kill -KILL "$in_env" "$in_argv" 2>/dev/null
    wait 2>/dev/null
fi

echo
echo "passed: $PASS   failed: $FAIL"
[ "$FAIL" -eq 0 ]
