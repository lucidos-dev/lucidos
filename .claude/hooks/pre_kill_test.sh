#!/bin/bash
# Tests for .claude/hooks/pre-kill.sh — the PreToolUse Bash hook that
# blocks commands which would kill the host engine.
# Run: ./.claude/hooks/pre_kill_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$SCRIPT_DIR/pre-kill.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# Run hook with the given env vars and a Bash command. Returns exit code; if
# it's 2, the hook blocked. Captures stderr in HOOK_STDERR and the exit code in
# HOOK_RC.
#
# Assertions below read HOOK_RC, never a bare `$?`. `$?` survives exactly one
# command: inside `if [ $? -eq 0 ]; then ... else fail "got $?"; fi` the `$?` in
# the else branch is the *test's* status (always 1), not the hook's — so every
# failure message reported "got 1" regardless of what the hook actually did.
run_hook() {
    local cmd="$1"
    local payload
    payload=$(jq -n --arg cmd "$cmd" '{tool_input: {command: $cmd}}')
    HOOK_STDERR=$(echo "$payload" | "$HOOK" 2>&1 >/dev/null)
    HOOK_RC=$?
    return "$HOOK_RC"
}

# ── Blocking cases ───────────────────────────────────────────────────────

test_blocks_direct_kill_of_host_pid() {
    echo "test: blocks 'kill <LUCIDOS_HOST_PID>'"
    export LUCIDOS_HOST_PID=12345
    unset LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "kill 12345"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected block (exit 2), got $HOOK_RC"
    fi
}

test_blocks_kill_dash_9_of_host_pid() {
    echo "test: blocks 'kill -9 <LUCIDOS_HOST_PID>'"
    export LUCIDOS_HOST_PID=12345
    unset LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "kill -9 12345"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2"
    fi
}

test_blocks_kill_dash_sigkill_of_host_pid() {
    echo "test: blocks 'kill -SIGTERM <LUCIDOS_HOST_PID>'"
    export LUCIDOS_HOST_PID=12345
    unset LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "kill -SIGTERM 12345"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2"
    fi
}

test_blocks_kill_of_frontend_pid() {
    echo "test: blocks 'kill <LUCIDOS_FRONTEND_PID>'"
    export LUCIDOS_FRONTEND_PID=67890
    unset LUCIDOS_HOST_PID LUCIDOS_API_PORT
    run_hook "kill 67890"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2"
    fi
}

test_blocks_pkill_f_lucidos_engine() {
    echo "test: blocks 'pkill -f lucidos-engine'"
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "pkill -f lucidos-engine"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2"
    fi
}

test_blocks_pkill_lucidos_engine() {
    echo "test: blocks 'pkill lucidos-engine'"
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "pkill lucidos-engine"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2"
    fi
}

test_blocks_killall_lucidos_engine() {
    echo "test: blocks 'killall lucidos-engine'"
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "killall lucidos-engine"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2"
    fi
}

test_blocks_lsof_xargs_kill_on_api_port() {
    echo "test: blocks 'lsof -ti :<api_port> | xargs kill'"
    export LUCIDOS_API_PORT=3000
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID
    run_hook "lsof -ti :3000 | xargs kill"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2"
    fi
}

test_blocks_lsof_xargs_kill_dash_9_on_api_port() {
    echo "test: blocks 'lsof -ti :<api_port> | xargs kill -9'"
    export LUCIDOS_API_PORT=3000
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID
    run_hook "lsof -ti :3000 | xargs kill -9"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2"
    fi
}

# ── Allowed cases ────────────────────────────────────────────────────────

test_allows_kill_of_unrelated_pid() {
    echo "test: allows 'kill 99999' (not the host)"
    export LUCIDOS_HOST_PID=12345
    unset LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "kill 99999"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed"
    else
        fail "expected exit 0, got $HOOK_RC — stderr: $HOOK_STDERR"
    fi
}

test_allows_bare_lsof_on_api_port() {
    echo "test: allows bare 'lsof -ti :<api_port>' (no xargs kill)"
    export LUCIDOS_API_PORT=3000
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID
    run_hook "lsof -ti :3000"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed"
    else
        fail "expected exit 0, got $HOOK_RC — stderr: $HOOK_STDERR"
    fi
}

test_allows_lsof_xargs_kill_on_other_port() {
    echo "test: allows 'lsof -ti :5555 | xargs kill' (not the host port)"
    export LUCIDOS_API_PORT=3000
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID
    run_hook "lsof -ti :5555 | xargs kill"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed"
    else
        fail "expected exit 0, got $HOOK_RC — stderr: $HOOK_STDERR"
    fi
}

test_allows_unrelated_bash() {
    echo "test: allows unrelated bash (ls, echo, etc.)"
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "echo hello && ls /tmp"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed"
    else
        fail "expected exit 0, got $HOOK_RC — stderr: $HOOK_STDERR"
    fi
}

test_allows_pkill_of_unrelated_process() {
    echo "test: allows 'pkill -f cargo' (not the engine)"
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "pkill -f cargo"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed"
    else
        fail "expected exit 0, got $HOOK_RC — stderr: $HOOK_STDERR"
    fi
}

test_no_env_vars_allows_kill_random_pid() {
    echo "test: with no env vars, 'kill 12345' is allowed (no host to protect)"
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "kill 12345"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed"
    else
        fail "expected exit 0, got $HOOK_RC — stderr: $HOOK_STDERR"
    fi
}

test_does_not_falsely_match_pid_substring() {
    echo "test: 'kill 123456' is not blocked when LUCIDOS_HOST_PID=12345"
    # 12345 is a substring of 123456 — the regex must use word boundaries
    # so it doesn't false-positive.
    export LUCIDOS_HOST_PID=12345
    unset LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "kill 123456"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed (no substring match)"
    else
        fail "false positive: blocked 'kill 123456' for host pid 12345 — stderr: $HOOK_STDERR"
    fi
}

test_allows_pkill_mention_in_quoted_string() {
    echo "test: 'git commit -m \"... pkill -f lucidos-engine ...\"' is allowed"
    # Regression: the commit message that introduced this hook contained the
    # literal string `pkill -f lucidos-engine` in its body, and the original
    # regex matched it even though `pkill` was inside quoted text rather than
    # being run. The CMD_START anchor in pre-kill.sh fixes this.
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "git commit -m 'docs: explain that pkill -f lucidos-engine is dangerous'"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed (quoted prose mention)"
    else
        fail "false positive on prose mention — stderr: $HOOK_STDERR"
    fi
}

test_allows_killall_mention_in_quoted_string() {
    echo "test: 'git commit -m \"... killall lucidos-engine ...\"' is allowed"
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "git commit -m 'docs: avoid killall lucidos-engine'"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed (quoted prose mention)"
    else
        fail "false positive on prose mention — stderr: $HOOK_STDERR"
    fi
}

test_blocks_pkill_inside_command_substitution() {
    echo "test: blocks 'echo \$(pkill -f lucidos-engine)' (command substitution)"
    # The model could try to wrap the kill in $(...) — CMD_START includes
    # `(` so the pattern matches at that boundary.
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    # shellcheck disable=SC2016 # the literal $(...) IS the payload under test — it must not expand here
    run_hook 'echo $(pkill -f lucidos-engine)'
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked inside command substitution"
    else
        fail "expected exit 2 inside \$(...) — stderr: $HOOK_STDERR"
    fi
}

test_blocks_pkill_after_pipe() {
    echo "test: blocks 'true | pkill -f lucidos-engine'"
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "true | pkill -f lucidos-engine"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked after pipe"
    else
        fail "expected exit 2 after pipe — stderr: $HOOK_STDERR"
    fi
}

test_blocks_absolute_path_kill_of_host_pid() {
    echo "test: blocks '/bin/kill <LUCIDOS_HOST_PID>' (path-prefixed bypass)"
    # Regression: pre-rev, the hook anchored `kill` directly after CMD_START
    # with no allowance for `/bin/`, so the model could trivially evade by
    # typing the absolute path.
    export LUCIDOS_HOST_PID=12345
    unset LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "/bin/kill 12345"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2 — /bin/kill bypassed the hook"
    fi
}

test_blocks_absolute_path_pkill_lucidos_engine() {
    echo "test: blocks '/usr/bin/pkill -f lucidos-engine' (path-prefixed bypass)"
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook "/usr/bin/pkill -f lucidos-engine"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2 — /usr/bin/pkill bypassed the hook"
    fi
}

test_blocks_pkill_after_newline() {
    echo "test: blocks 'echo first<NL>pkill -f lucidos-engine' (multi-line script)"
    # Regression: pre-rev, CMD_START matched the literal escape `\n` (backslash
    # + n) inside the character class instead of a real newline, so a pkill on
    # its own line slipped past the anchor.
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID LUCIDOS_API_PORT
    run_hook $'echo first\npkill -f lucidos-engine'
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked after newline"
    else
        fail "expected exit 2 — newline-separated pkill bypassed the hook"
    fi
}

# ── Run all ──────────────────────────────────────────────────────────────

test_blocks_direct_kill_of_host_pid
test_blocks_kill_dash_9_of_host_pid
test_blocks_kill_dash_sigkill_of_host_pid
test_blocks_kill_of_frontend_pid
test_blocks_pkill_f_lucidos_engine
test_blocks_pkill_lucidos_engine
test_blocks_killall_lucidos_engine
test_blocks_lsof_xargs_kill_on_api_port
test_blocks_lsof_xargs_kill_dash_9_on_api_port
test_allows_kill_of_unrelated_pid
test_allows_bare_lsof_on_api_port
test_allows_lsof_xargs_kill_on_other_port
test_allows_unrelated_bash
test_allows_pkill_of_unrelated_process
test_no_env_vars_allows_kill_random_pid
test_does_not_falsely_match_pid_substring
test_allows_pkill_mention_in_quoted_string
test_allows_killall_mention_in_quoted_string
test_blocks_pkill_inside_command_substitution
test_blocks_pkill_after_pipe
test_blocks_absolute_path_kill_of_host_pid
test_blocks_absolute_path_pkill_lucidos_engine
test_blocks_pkill_after_newline

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
