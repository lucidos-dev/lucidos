#!/bin/bash
# Tests for scripts/lib/memory_watch.sh, the host memory watch that records a
# runaway process while it is alive and kills one that outgrows the host's RAM.
# Run: ./scripts/lib/memory_watch_test.sh   (no harness; direct, like host_memory_guard_test.sh)
#
# Hermetic by construction, and that is load-bearing: this library's job is to
# list and kill real processes. Every host read goes through a seam fed from a
# synthetic table. The real top, ps, lsof, launchctl and kill are shadowed too.
# A stray call is recorded and fails the suite instead of reaching the host
# (ADR 0025). An empty synthetic feed means "no processes", never a fall-back.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SANDBOX="$(mktemp -d)"
cleanup() { rm -rf "$SANDBOX"; }
trap cleanup EXIT

export MEMORY_WATCH_DIR="$SANDBOX/watch"
LOG="$MEMORY_WATCH_DIR/memory-watch.log"

# shellcheck source=memory_watch.sh
source "$SCRIPT_DIR/memory_watch.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }

assert_eq() {
    local expected="$1" actual="$2" msg="$3"
    if [ "$expected" = "$actual" ]; then
        pass "$msg"
    else
        fail "$msg (expected '$expected', got '$actual')"
    fi
}

assert_says() {
    local file="$1" needle="$2" msg="$3"
    if [ -f "$file" ] && grep -qF -- "$needle" "$file"; then
        pass "$msg"
    else
        fail "$msg (no '$needle')"
        [ -f "$file" ] && sed 's/^/      | /' "$file"
    fi
}

assert_silent_about() {
    local file="$1" needle="$2" msg="$3"
    if [ -f "$file" ] && grep -qF -- "$needle" "$file"; then
        fail "$msg (found '$needle')"
        sed 's/^/      | /' "$file"
    else
        pass "$msg"
    fi
}

# ── the synthetic host ──────────────────────────────────────────────────
# SYN_TOP is what `top` printed. SYN_PS is one tab-separated row per process:
# pid, ppid, pgid, uid, start time, command. SYN_CWD maps pid to cwd.
SYN_TOP=""
SYN_PS="$SANDBOX/ps.tsv"
SYN_CWD="$SANDBOX/cwd.tsv"
SYN_RAM=51539607552
CALLS="$SANDBOX/calls"
REAL="$SANDBOX/real-calls"
KILLS="$SANDBOX/kills"
ALIVE="$SANDBOX/alive"
PROTECTED="$SANDBOX/protected"
LOGGED_AT_KILL="$SANDBOX/logged-at-kill"
MY_UID="$(id -u)"
: > "$REAL"

# shellcheck disable=SC2329 # a seam: invoked by the sourced library, not from this file
_mw_top() { echo top >> "$CALLS"; printf '%s' "$SYN_TOP"; }
# SYN_TOP_PID is what `top -pid` printed just before a kill; empty means SYN_TOP.
SYN_TOP_PID=""
# shellcheck disable=SC2329 # a seam: invoked by the sourced library, not from this file
_mw_top_pid() { echo "top-pid $1" >> "$CALLS"; printf '%s' "${SYN_TOP_PID:-$SYN_TOP}"; }
# shellcheck disable=SC2329 # a seam: invoked by the sourced library, not from this file
_mw_proc_info() { echo "ps $1" >> "$CALLS"; awk -F'\t' -v p="$1" '$1 == p' "$SYN_PS"; }
# shellcheck disable=SC2329 # a seam: invoked by the sourced library, not from this file
_mw_cwd() { echo "cwd $1" >> "$CALLS"; awk -F'\t' -v p="$1" '$1 == p { print $2 }' "$SYN_CWD"; }
# shellcheck disable=SC2329 # a seam: invoked by the sourced library, not from this file
_mw_physmem_bytes() { printf '%s' "$SYN_RAM"; }
# shellcheck disable=SC2329 # a seam: invoked by the sourced library, not from this file
_mw_wait() { echo "wait $1" >> "$SANDBOX/waits"; }
# shellcheck disable=SC2329 # a seam: invoked by the sourced library, not from this file
_mw_now() { printf '%s' "2026-09-24T06:00:00+0200"; }
# shellcheck disable=SC2329 # a seam: invoked by the sourced library, not from this file
is_protected_host_pid() { [ "$1" -le 1 ] || grep -qx "$1" "$PROTECTED"; }

# The real commands, shadowed so a missed seam is caught rather than obeyed.
# shellcheck disable=SC2329 # shadows a host command for the library under test
top() { echo "top $*" >> "$REAL"; }
# shellcheck disable=SC2329 # shadows a host command for the library under test
ps() { echo "ps $*" >> "$REAL"; }
# shellcheck disable=SC2329 # shadows a host command for the library under test
lsof() { echo "lsof $*" >> "$REAL"; }

# The kill shim. SIGTERM leaves a process alive, so the SIGKILL arm is reached.
# A pid the test did not plant is refused and fails the suite.
# shellcheck disable=SC2329 # shadows the builtin for the library under test
kill() {
    local sig="$1" pid="$2"
    if ! grep -qx "$pid" "$ALIVE.planted"; then
        echo "REFUSED $sig $pid" >> "$KILLS"
        return 1
    fi
    case "$sig" in
        -0) grep -qx "$pid" "$ALIVE" ;;
        *)
            grep -c "OVER pid=$pid " "$LOG" 2>/dev/null >> "$LOGGED_AT_KILL"
            echo "$sig $pid" >> "$KILLS"
            if [ "$sig" = "-KILL" ]; then
                grep -vx "$pid" "$ALIVE" > "$ALIVE.new"
                mv "$ALIVE.new" "$ALIVE"
            fi
            return 0
            ;;
    esac
}

# A top listing in the shape `top -l 1 -o mem -stats pid,mem` prints.
top_of() {
    printf 'Processes: 700 total\nPhysMem: 47G used\n\nPID    MEM\n'
    printf '%s\n' "$@"
}

reset_host() {
    rm -rf "$MEMORY_WATCH_DIR"
    : > "$CALLS"
    : > "$KILLS"
    : > "$PROTECTED"
    : > "$LOGGED_AT_KILL"
    SYN_RAM=51539607552
    SYN_TOP_PID=""
    unset MEMORY_WATCH_LOG_PCT MEMORY_WATCH_KILL_PCT MEMORY_WATCH_LOG_MAX_KB
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
        626 600 600 "$MY_UID" "Wed Sep 23 18:52:08 2026" "grep -r needle /data" \
        600 580 600 "$MY_UID" "Wed Sep 23 18:52:07 2026" "bash -c grep -r needle /data | head" \
        580 1 580 "$MY_UID" "Wed Sep 23 18:40:00 2026" "node /usr/local/bin/claude" \
        1 0 1 0 "Sun Sep 20 05:40:00 2026" "/sbin/launchd" \
        700 1 700 "$MY_UID" "Wed Sep 23 17:00:00 2026" "grep 261G runaway notes" \
        800 1 800 0 "Wed Sep 23 17:00:00 2026" "/usr/libexec/rootd" \
        > "$SYN_PS"
    printf '%s\t%s\n' 626 "/Users/me/project" > "$SYN_CWD"
    printf '%s\n' 626 600 580 700 800 > "$ALIVE"
    cp "$ALIVE" "$ALIVE.planted"
}

# ── the tests ───────────────────────────────────────────────────────────

test_top_units_parse() {
    echo "test: top's MEM column parses to bytes"
    local parsed="$SANDBOX/parsed"
    top_of "1 90G" "2 7101M" "3 657M+" "4 12K" "5 0B" "6 1.5G-" "7 3T" | _mw_parse_top > "$parsed"
    assert_says "$parsed" "2 7445938176" "M is mebibytes"
    assert_says "$parsed" "3 688914432" "a trailing + is ignored"
    assert_says "$parsed" "4 12288" "K is kibibytes"
    assert_eq "5 0" "$(grep '^5 ' "$parsed")" "B is bytes"
    assert_says "$parsed" "6 1610612736" "a decimal G parses, and a trailing - is ignored"
    assert_says "$parsed" "7 3298534883328" "T is tebibytes"
}

test_an_over_threshold_process_is_recorded_in_full() {
    echo "test: a process over the log threshold is recorded with its origin"
    reset_host
    SYN_TOP="$(top_of "626 20G" "700 500M")"
    memory_watch_once >"$SANDBOX/once.out" 2>&1
    assert_says "$LOG" "OVER pid=626 footprint=20.00 GB" "the record names the pid and its footprint"
    assert_says "$LOG" "started: Wed Sep 23 18:52:08 2026" "the record has the start time"
    assert_says "$LOG" "ppid: 600  pgid: 600  uid: $MY_UID" "the record has ppid, pgid and uid"
    assert_says "$LOG" "command: grep -r needle /data" "the record has the full command line"
    assert_says "$LOG" "cwd: /Users/me/project" "the record has the working directory"
    assert_says "$LOG" "parents: 600 bash <- 580 node <- 1 launchd" "the record has the parent chain up to launchd"
    assert_silent_about "$LOG" "pid=700" "a process under the threshold is not recorded"
}

test_a_process_already_recorded_gets_a_short_line() {
    echo "test: a second tick adds one line, not a second record"
    reset_host
    SYN_TOP="$(top_of "626 20G")"
    memory_watch_once >/dev/null 2>&1
    SYN_TOP="$(top_of "626 24G")"
    memory_watch_once >/dev/null 2>&1
    assert_eq "1" "$(grep -c 'OVER pid=626 ' "$LOG")" "the full record is written once"
    assert_says "$LOG" "STILL pid=626 footprint=24.00 GB" "the next tick records the growth"
    # The same pid number with a new start time is a new process.
    awk -F'\t' 'BEGIN { OFS = "\t" } $1 == 626 { $5 = "Wed Sep 23 23:00:00 2026" } { print }' "$SYN_PS" > "$SYN_PS.new"
    mv "$SYN_PS.new" "$SYN_PS"
    memory_watch_once >/dev/null 2>&1
    assert_eq "2" "$(grep -c 'OVER pid=626 ' "$LOG")" "a recycled pid gets its own full record"
}

test_a_process_past_ram_is_recorded_then_killed() {
    echo "test: a process past physical RAM is recorded, then killed"
    reset_host
    SYN_TOP="$(top_of "626 60G")"
    memory_watch_once >"$SANDBOX/kill.out" 2>&1
    assert_says "$KILLS" "-TERM 626" "the runaway gets SIGTERM first"
    assert_says "$KILLS" "-KILL 626" "and SIGKILL when it is still alive"
    assert_silent_about "$KILLS" "REFUSED" "no unplanted pid was signalled"
    assert_eq "1" "$(head -1 "$LOGGED_AT_KILL")" "the record was on disk before the first signal"
    assert_says "$LOG" "KILL pid=626" "the kill is logged"
    assert_silent_about "$KILLS" " 600" "its parent is not signalled"
}

test_a_process_that_shrank_before_the_kill_lives() {
    echo "test: the kill re-measures, and a process now under the line is left alone"
    reset_host
    SYN_TOP="$(top_of "626 60G")"
    SYN_TOP_PID="$(top_of "626 30G")"
    memory_watch_once >/dev/null 2>&1
    if [ -s "$KILLS" ]; then fail "a process that shrank was signalled: $(tr '\n' ' ' < "$KILLS")"; else pass "it is not signalled"; fi
    assert_says "$LOG" "not killed: pid 626 is no longer past the kill threshold" "the log says why"
}

test_a_reused_pid_is_never_killed() {
    echo "test: a pid whose start time changed since the reading is left alone"
    reset_host
    SYN_TOP="$(top_of "626 60G")"
    (
        # The second look at pid 626 finds a different process behind the number.
        # shellcheck disable=SC2329 # a seam: invoked by the sourced library, not from this file
        _mw_proc_info() {
            echo "ps $1" >> "$CALLS"
            if [ "$1" = 626 ] && [ "$(grep -c '^ps 626$' "$CALLS")" -gt 1 ]; then
                printf '626\t1\t626\t%s\tThu Sep 24 06:00:00 2026\tsomething else\n' "$MY_UID"
                return 0
            fi
            awk -F'\t' -v p="$1" '$1 == p' "$SYN_PS"
        }
        memory_watch_once
    ) >/dev/null 2>&1
    if [ -s "$KILLS" ]; then fail "a reused pid was signalled: $(tr '\n' ' ' < "$KILLS")"; else pass "it is not signalled"; fi
    assert_says "$LOG" "not killed: pid 626 is no longer the process that was measured" "the log says why"
}

test_a_process_between_the_thresholds_is_only_recorded() {
    echo "test: 30 GB on a 48 GB host is recorded and left alone"
    reset_host
    SYN_TOP="$(top_of "626 30G")"
    memory_watch_once >/dev/null 2>&1
    assert_says "$LOG" "OVER pid=626" "it is recorded"
    if [ -s "$KILLS" ]; then fail "it was signalled: $(tr '\n' ' ' < "$KILLS")"; else pass "it is not signalled"; fi
}

test_the_kill_can_be_disabled() {
    echo "test: MEMORY_WATCH_KILL_PCT=0 records and never kills"
    reset_host
    SYN_TOP="$(top_of "626 60G")"
    MEMORY_WATCH_KILL_PCT=0 memory_watch_once >/dev/null 2>&1
    if [ -s "$KILLS" ]; then fail "a disabled kill signalled: $(tr '\n' ' ' < "$KILLS")"; else pass "nothing is signalled"; fi
    assert_says "$LOG" "OVER pid=626" "the runaway is still recorded"
}

test_a_protected_or_foreign_process_is_never_killed() {
    echo "test: a protected pid and another user's pid are recorded, never killed"
    reset_host
    echo 626 > "$PROTECTED"
    SYN_TOP="$(top_of "626 60G" "800 60G")"
    memory_watch_once >/dev/null 2>&1
    if [ -s "$KILLS" ]; then fail "a protected or foreign pid was signalled: $(tr '\n' ' ' < "$KILLS")"; else pass "neither is signalled"; fi
    assert_says "$LOG" "not killed: pid 626 is protected" "the log says why the protected one lives"
    assert_says "$LOG" "not killed: pid 800 belongs to uid 0" "the log says why the foreign one lives"
}

test_pid_0_and_1_are_never_candidates() {
    echo "test: kernel_task and launchd are skipped outright"
    reset_host
    SYN_TOP="$(top_of "0 90G" "1 60G")"
    memory_watch_once >/dev/null 2>&1
    assert_silent_about "$LOG" "OVER" "neither is recorded"
    if [ -s "$KILLS" ]; then fail "pid 0 or 1 was signalled"; else pass "neither is signalled"; fi
}

test_selection_is_by_footprint_not_command_line() {
    echo "test: a small process whose command names a runaway is ignored"
    reset_host
    SYN_TOP="$(top_of "700 200M")"
    memory_watch_once >/dev/null 2>&1
    assert_silent_about "$LOG" "pid=700" "it is not recorded"
}

test_an_unreadable_host_fails_open() {
    echo "test: no top output, or output with no header, means a note and no kill"
    local rc=0 feed
    for feed in "" "garbage with no header"; do
        reset_host
        SYN_TOP="$feed"
        rc=0
        memory_watch_once >/dev/null 2>&1 || rc=$?
        assert_eq "0" "$rc" "the tick exits 0 on '${feed:-empty}'"
        assert_says "$LOG" "note: top gave no reading" "the log notes the missing reading"
        if [ -s "$KILLS" ]; then fail "an unreadable host produced a kill"; else pass "nothing is signalled"; fi
    done
}

test_a_clean_host_costs_one_top_call() {
    echo "test: with nothing over the threshold, a tick runs top once and nothing else"
    reset_host
    SYN_TOP="$(top_of "626 2G" "700 500M")"
    memory_watch_once >/dev/null 2>&1
    assert_eq "top" "$(tr '\n' ' ' < "$CALLS" | sed 's/ $//')" "only top was called"
}

test_the_log_is_bounded() {
    echo "test: the log rotates once at its cap and keeps one previous file"
    local size
    reset_host
    SYN_TOP="$(top_of "626 20G")"
    for _ in $(seq 1 40); do
        rm -f "$MEMORY_WATCH_DIR/seen"
        MEMORY_WATCH_LOG_MAX_KB=1 memory_watch_once >/dev/null 2>&1
    done
    size="$(wc -c < "$LOG" | tr -d ' ')"
    if [ "$size" -le 2048 ]; then pass "the live log stays near the cap ($size bytes)"; else fail "the live log grew to $size bytes"; fi
    if [ -f "$LOG.1" ]; then pass "one previous file is kept"; else fail "no rotated file"; fi
    if [ -f "$LOG.2" ]; then fail "more than one previous file is kept"; else pass "only one previous file"; fi
}

test_garbage_knobs_fall_back() {
    echo "test: a knob that is not a number falls back to its default"
    assert_eq "20" "$(MEMORY_WATCH_LOG_PCT=abc _mw_log_pct)" "the log share falls back to 20"
    assert_eq "100" "$(MEMORY_WATCH_KILL_PCT=1e9 _mw_kill_pct)" "the kill share falls back to 100"
    assert_eq "0" "$(MEMORY_WATCH_KILL_PCT=0 _mw_kill_pct)" "0 is honoured as off"
    assert_eq "100" "$(MEMORY_WATCH_KILL_PCT=10 _mw_kill_pct)" "a share under half of RAM falls back"
    assert_eq "80" "$(MEMORY_WATCH_KILL_PCT=80 _mw_kill_pct)" "a share over half of RAM is honoured"
}

# ── the installer ───────────────────────────────────────────────────────
LAUNCHCTL_CALLS="$SANDBOX/launchctl-calls"
# A loaded agent answers `print` until the preceding bootout has landed.
LOADED_PRINTS=0
# shellcheck disable=SC2329 # a seam: invoked by the sourced library, not from this file
_mw_launchctl() {
    echo "$*" >> "$LAUNCHCTL_CALLS"
    if [ "$1" = print ]; then
        [ "$LOADED_PRINTS" -gt 0 ] || return 1
        LOADED_PRINTS=$((LOADED_PRINTS - 1))
    fi
    return 0
}
# shellcheck disable=SC2329 # shadows a host command for the library under test
launchctl() { echo "launchctl $*" >> "$REAL"; }

test_the_installer_refuses_a_worktree() {
    echo "test: a checkout inside a coding-agent worktree is refused"
    local agents="$SANDBOX/agents-wt" rc=0
    : > "$LAUNCHCTL_CALLS"
    memory_watch_install "/Users/me/workspaces/dev/.lucidos/worktrees/thread-1234" "$agents" \
        >"$SANDBOX/wt.out" 2>&1 || rc=$?
    assert_eq "1" "$rc" "the install fails"
    assert_says "$SANDBOX/wt.out" "coding-agent worktree" "it says why"
    if [ -e "$agents/$MEMORY_WATCH_LABEL.plist" ]; then fail "a plist was written"; else pass "no plist is written"; fi
    if [ -s "$LAUNCHCTL_CALLS" ]; then fail "launchctl was called"; else pass "launchctl is never called"; fi
}

test_the_installer_writes_a_plist_for_the_checkout() {
    echo "test: an install writes the agent and loads it, and a second one replaces it"
    local agents="$SANDBOX/agents" plist
    : > "$LAUNCHCTL_CALLS"
    memory_watch_install "/Users/me/projects/lucidos" "$agents" >"$SANDBOX/inst.out" 2>&1
    plist="$agents/$MEMORY_WATCH_LABEL.plist"
    assert_says "$plist" "<string>/Users/me/projects/lucidos/scripts/memory-watch.sh</string>" "the agent runs the checkout's own script"
    assert_says "$plist" "<string>--once</string>" "one tick per run"
    assert_says "$plist" "<integer>30</integer>" "every 30 seconds"
    assert_silent_about "$plist" "/.lucidos/worktrees/" "no worktree path"
    assert_says "$plist" "<key>MEMORY_WATCH_KILL_PCT</key>" "the knobs travel to the agent"
    assert_says "$plist" "<string>$MEMORY_WATCH_DIR</string>" "the agent logs where the installer said"
    assert_says "$LAUNCHCTL_CALLS" "bootstrap gui/$MY_UID $plist" "the agent is loaded"
    : > "$SANDBOX/waits"
    LOADED_PRINTS=2
    memory_watch_install "/Users/me/projects/lucidos" "$agents" >/dev/null 2>&1
    assert_eq "2" "$(wc -l < "$SANDBOX/waits" | tr -d ' ')" "a reinstall waits while launchd still holds the old agent"
    assert_eq "2" "$(grep -c '^bootout ' "$LAUNCHCTL_CALLS")" "each install first unloads any previous agent"
    assert_eq "2" "$(grep -c '^bootstrap ' "$LAUNCHCTL_CALLS")" "and loads the new one"
}

test_the_plist_escapes_xml_and_carries_the_knobs() {
    echo "test: a path with XML characters stays valid, and a knob set at install is kept"
    local plist="$SANDBOX/escaped.plist"
    MEMORY_WATCH_KILL_PCT=0 memory_watch_plist "/Users/me/a&b<c>" > "$plist"
    assert_says "$plist" "/Users/me/a&amp;b&lt;c&gt;/scripts/memory-watch.sh" "the path is escaped"
    assert_eq "0" "$(grep -A1 'MEMORY_WATCH_KILL_PCT' "$plist" | sed -n 's:.*<string>\(.*\)</string>.*:\1:p')" \
        "killing turned off at install stays off in the agent"
}

test_the_uninstaller_removes_the_agent() {
    echo "test: an uninstall unloads the agent and removes its plist"
    local agents="$SANDBOX/agents"
    : > "$LAUNCHCTL_CALLS"
    memory_watch_uninstall "$agents" >/dev/null 2>&1
    if [ -e "$agents/$MEMORY_WATCH_LABEL.plist" ]; then fail "the plist survived"; else pass "the plist is removed"; fi
    assert_says "$LAUNCHCTL_CALLS" "bootout gui/$MY_UID/$MEMORY_WATCH_LABEL" "the agent is unloaded"
}

test_top_units_parse
test_an_over_threshold_process_is_recorded_in_full
test_a_process_already_recorded_gets_a_short_line
test_a_process_past_ram_is_recorded_then_killed
test_a_process_between_the_thresholds_is_only_recorded
test_the_kill_can_be_disabled
test_a_protected_or_foreign_process_is_never_killed
test_pid_0_and_1_are_never_candidates
test_selection_is_by_footprint_not_command_line
test_an_unreadable_host_fails_open
test_a_clean_host_costs_one_top_call
test_the_log_is_bounded
test_garbage_knobs_fall_back
test_the_installer_refuses_a_worktree
test_the_installer_writes_a_plist_for_the_checkout
test_the_plist_escapes_xml_and_carries_the_knobs
test_the_uninstaller_removes_the_agent
test_a_process_that_shrank_before_the_kill_lives
test_a_reused_pid_is_never_killed

echo "test: no real host command was reached"
if [ -s "$REAL" ]; then
    fail "a real host command ran: $(tr '\n' ' ' < "$REAL")"
else
    pass "every host read went through a seam"
fi

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
