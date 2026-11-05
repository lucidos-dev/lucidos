#!/bin/bash
# Asserts that every hook `.claude/settings.json` registers actually resolves to
# a file that exists and is executable, and that `settings.json` is valid JSON.
#
# WHY THIS EXISTS. `log-instructions-loaded.sh` was committed at mode 100644
# while every other hook is 100755, so Claude Code could not exec it. Two
# properties of hooks made that invisible: several events ignore the hook's exit
# code by design (`InstructionsLoaded` is one), and hook stdout does not reach
# the transcript. The permission error therefore went nowhere at all. The only
# symptom was a log file that never appeared, which nobody looks for until they
# need it to diagnose something else. A hook that silently never runs looks
# exactly like a hook that was never added, which is the same failure shape as
# the `globs:` scoping bug that `scripts/check-context-budget.sh` guards.
#
# Scope is deliberately narrow: this checks that a registered hook CAN run, not
# what it does. Each hook's own behaviour is its own test's job (see
# `.claude/hooks/pre_kill_test.sh`, `scripts/lib/em_dash_scan_test.sh`).
#
# Only project hooks are in scope. The engine generates a second settings file
# at `<workspace>/.lucidos/cc-settings.json` whose commands are `lucidos`
# subcommands resolved from PATH, not repo paths; those are covered by
# `cc_settings.rs`'s own unit tests.
#
# Run: ./scripts/lib/hooks_registered_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SETTINGS="$REPO_ROOT/.claude/settings.json"

PASS=0
FAIL=0
pass() {
    echo "  ok:   $*"
    PASS=$((PASS + 1))
}
fail() {
    echo "  FAIL: $*"
    FAIL=$((FAIL + 1))
}

echo "hooks registered in .claude/settings.json:"

if [ ! -r "$SETTINGS" ]; then
    echo "  FAIL: $SETTINGS is not readable"
    exit 1
fi

if python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$SETTINGS" 2>/dev/null; then
    pass "settings.json is valid JSON"
else
    fail "settings.json is not valid JSON"
    exit 1
fi

# Every hook command string, one per line. Walks all events and matchers rather
# than naming them, so a hook added under a NEW event is covered the day it
# lands: the whole point is that nobody has to remember this file exists.
COMMANDS="$(python3 - "$SETTINGS" <<'PY'
import json, sys

with open(sys.argv[1]) as fh:
    settings = json.load(fh)

for entries in (settings.get("hooks") or {}).values():
    for entry in entries or []:
        for hook in entry.get("hooks") or []:
            if hook.get("type") == "command" and hook.get("command"):
                print(hook["command"])
PY
)"

if [ -z "$COMMANDS" ]; then
    fail "no command hooks found at all, so this test verified nothing"
    exit 1
fi

COUNT=0
while IFS= read -r cmd; do
    [ -n "$cmd" ] || continue
    COUNT=$((COUNT + 1))

    # Registered commands are shell strings. Resolve the repo-relative form the
    # project uses, `"$CLAUDE_PROJECT_DIR"/path`, into a real path; anything
    # else is a bare command resolved from PATH and out of scope here.
    # shellcheck disable=SC2016 # the literal text "$CLAUDE_PROJECT_DIR" is what we match: it is Claude Code's own placeholder, unexpanded inside the command string settings.json stores
    # shellcheck disable=SC2016 # the pattern below matches the LITERAL "$CLAUDE_PROJECT_DIR" as written in settings.json; expanding it is the bug
    case "$cmd" in
        *'$CLAUDE_PROJECT_DIR'*)
            rel="${cmd#*\$CLAUDE_PROJECT_DIR}"
            rel="${rel#\"}"
            rel="${rel#/}"
            path="$REPO_ROOT/$rel"
            ;;
        /*)
            path="$cmd"
            ;;
        *)
            pass "$cmd resolves from PATH (not a repo file, skipped)"
            continue
            ;;
    esac

    name="${path#"$REPO_ROOT"/}"
    if [ ! -f "$path" ]; then
        fail "$name is registered as a hook but does not exist"
        continue
    fi
    if [ ! -x "$path" ]; then
        fail "$name is registered as a hook but is not executable (chmod +x it)"
        continue
    fi
    pass "$name exists and is executable"
done <<EOF
$COMMANDS
EOF

echo
echo "checked $COUNT registered command hook(s)"
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ]
