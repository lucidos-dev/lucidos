#!/bin/bash
# Tests for scripts/lib/ps_tools.sh and its stand-in, scripts/lib/bin/ps.
#
# Hermetic: the suite only reads processes, and the one it reads and stops is
# a `sleep` it spawned itself. The switch is tested through its probe seam.
# Where /bin/ps runs, the stand-in must print what it prints, field by field;
# inside the sandbox that comparison is skipped and the fixed facts still hold.
#
# Run: ./scripts/lib/ps_tools_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/ps_tools.sh
source "$SCRIPT_DIR/ps_tools.sh"
STAND_IN="$SCRIPT_DIR/bin/ps"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
assert_eq() { # <expected> <actual> <label>
    if [ "$1" = "$2" ]; then pass "$3"; else fail "$3 (expected '$1', got '$2')"; fi
}

# squeeze: trim and collapse blanks, since column widths are not the contract.
squeeze() { sed 's/^ *//; s/ *$//; s/  */ /g'; }

echo "ps_tools_env"
REAL_PATH="$PATH"
uname() { echo Darwin; }
_ps_tools_system_ps_runs() { return 1; }
ps_tools_env
assert_eq "$_PS_TOOLS_BIN:$REAL_PATH" "$PATH" "a macOS host whose ps cannot run gets the stand-in first"
ps_tools_env
assert_eq "$_PS_TOOLS_BIN:$REAL_PATH" "$PATH" "a second call adds nothing"
PATH="$REAL_PATH"
_ps_tools_system_ps_runs() { return 0; }
ps_tools_env
assert_eq "$REAL_PATH" "$PATH" "a host whose ps runs keeps it"
uname() { echo Linux; }
_ps_tools_system_ps_runs() { return 1; }
ps_tools_env
assert_eq "$REAL_PATH" "$PATH" "a Linux host is never switched"
PATH="$REAL_PATH"
unset -f uname

if [ "$(uname -s)" != Darwin ]; then
    echo ""
    echo "Skipping the stand-in: it reads macOS's libproc."
    echo ""
    echo "Passed: $PASS  Failed: $FAIL"
    [ "$FAIL" -eq 0 ]
    exit
fi

sleep 60 &
SLEEPER=$!
trap 'kill "$SLEEPER" 2>/dev/null' EXIT

echo ""
echo "the stand-in"
assert_eq "$SLEEPER $$" "$("$STAND_IN" -p "$SLEEPER" -o pid=,ppid= | squeeze)" "reads a pid and its parent"
assert_eq "sleep 60" "$("$STAND_IN" -ww -p "$SLEEPER" -o command=)" "reads the command line"
assert_eq "sleep" "$("$STAND_IN" -p "$SLEEPER" -o comm=)" "reads argv[0] as comm"
LISTED="$("$STAND_IN" -Aww -o pid=,command= | squeeze | grep -c "^$SLEEPER sleep 60$")"
assert_eq 1 "$LISTED" "lists every process, this one included"
assert_eq "PID COMMAND" "$("$STAND_IN" -p "$SLEEPER" -o pid,command | head -1 | squeeze)" "prints a header unless every field names an empty one"

DEAD_OUT="$("$STAND_IN" -p 999999 -o pid= 2>&1)"
DEAD_RC=$?
assert_eq "1:" "$DEAD_RC:$DEAD_OUT" "a dead pid prints nothing and fails"
"$STAND_IN" -q -p "$SLEEPER" -o pid= >/dev/null 2>&1
assert_eq 2 $? "an option it does not implement is a usage error"
"$STAND_IN" -p "$SLEEPER" -o pcpu= >/dev/null 2>&1
assert_eq 2 $? "so is a field it does not implement"

echo ""
if /bin/ps -p $$ -o pid= >/dev/null 2>&1; then
    echo "agreement with /bin/ps"
    for field in pid ppid pgid uid lstart comm command args; do
        assert_eq "$(/bin/ps -ww -p "$SLEEPER" -o "$field=" | squeeze)" \
            "$("$STAND_IN" -ww -p "$SLEEPER" -o "$field=" | squeeze)" "$field"
    done
    assert_eq "$(/bin/ps -p "$SLEEPER" -o stat= | cut -c1)" "$("$STAND_IN" -p "$SLEEPER" -o stat=)" "stat's state letter"
else
    echo "Skipping agreement with /bin/ps: it cannot run here."
fi

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
