#!/bin/bash
# Tests for scripts/harden-other-model-review.sh: the other-model review
# `/harden` launches in Phase 1 and joins in Phase 3.
#
# Hermetic: HOME is a temp dir, and fake `node` and `codex` binaries lead
# PATH. A fake `claude` sits in each Claude case's HOME, and no system
# `claude` is searched. Each fake plays one scripted outcome per attempt.
# Nothing reaches a real CLI, its logs, or the network.
#
# Codex covers: a reviewer that fails twice with a 403, one that recovers on
# the retry, one that answers first time, a zero exit that still says nothing,
# a 403 only inside the JSON payload, the Codex log as the status source
# (fresh and stale), the base passed through, no plugin, and a CLI that never
# answers. Claude covers: findings, none, a retry that recovers, not signed
# in, a 403, an error subtype, no structured output, the timeout, no CLI, no
# `jq`, a CLI that never answers, the pinned argv and the diff it reads, and
# CLAUDE_CONFIG_DIR passed through untouched. Shared: the `decide` table, an unknown backend, the attempt
# timeout, the env scrub, and the task timer against the attempt budget.
#
# Run: ./scripts/lib/harden_other_model_review_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLI="$SCRIPT_DIR/../harden-other-model-review.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
expect_eq() { # <name> <expected> <actual>
    if [ "$2" = "$3" ]; then pass "$1"; else fail "$1: expected [$2], got [$3]"; fi
}
expect_has() { # <name> <needle> <haystack>
    case "$3" in
        *"$2"*) pass "$1" ;;
        *) fail "$1: [$2] not in [$3]" ;;
    esac
}
expect_lacks() { # <name> <needle> <haystack>
    case "$3" in
        *"$2"*) fail "$1: [$2] unexpectedly in [$3]" ;;
        *) pass "$1" ;;
    esac
}

MADE="$(mktemp -d)" || exit 1
[ -n "$MADE" ] && [ -d "$MADE" ] || exit 1
TMP="$(cd "$MADE" && pwd -P)" || exit 1
trap 'rm -rf "$TMP"' EXIT

GENERIC="Reviewer failed to output a response."
FORBIDDEN="unexpected status 403 Forbidden: Project does not have access to model gpt-6-astra"

mkdir -p "$TMP/bin"
# The fake reviewer. Each call takes the next line of $FAKE_PLAN as its
# outcome, records its arguments, and counts itself in $FAKE_CALLS.
cat > "$TMP/bin/node" << 'EOF'
#!/bin/bash
n=$(($(cat "$FAKE_CALLS" 2>/dev/null || echo 0) + 1))
echo "$n" > "$FAKE_CALLS"
printf '%s\n' "$*" >> "$FAKE_ARGS"
case "$(sed -n "${n}p" "$FAKE_PLAN")" in
    ok)
        env > "$HOME/env"
        printf '{\n  "codex": {\n    "status": 0,\n    "stdout": "No actionable bugs found."\n  }\n}\n'
        exit 0
        ;;
    hang)
        echo "reviewing" >&2
        sleep 30
        exit 0
        ;;
    fail403)
        printf '{\n  "codex": {\n    "status": 1,\n    "stderr": "",\n    "stdout": "%s"\n  }\n}\n' "$FAKE_GENERIC"
        echo "ERROR: $FAKE_FORBIDDEN" >&2
        exit 1
        ;;
    quotes)
        printf '{\n  "codex": {\n    "status": 0,\n    "stdout": "harden.md: the doc names \\"%s\\" as a failure."\n  }\n}\n' "$FAKE_GENERIC"
        exit 0
        ;;
    silent0)
        printf '{\n  "codex": {\n    "status": 0,\n    "stdout": "%s"\n  }\n}\n' "$FAKE_GENERIC"
        exit 0
        ;;
    json403)
        printf '{\n  "codex": {\n    "status": 1,\n    "stderr": "turn failed\\nerror: {\\"status\\": \\"%s\\"}",\n    "stdout": "%s"\n  }\n}\n' "$FAKE_FORBIDDEN" "$FAKE_GENERIC"
        exit 1
        ;;
    bare)
        printf '{\n  "codex": {\n    "status": 1,\n    "stdout": "%s"\n  }\n}\n' "$FAKE_GENERIC"
        exit 1
        ;;
    empty)
        printf '{\n  "codex": {\n    "status": 0,\n    "stderr": "",\n    "stdout": ""\n  }\n}\n'
        exit 0
        ;;
    logged-multiline)
        sqlite3 "$HOME/.codex/logs_2.sqlite" \
            "insert into logs (ts, thread_id, feedback_log_body) values ($(date +%s), 't-own', 'turn sampling_error=403 Forbidden' || char(10) || 'at responses');"
        printf '{\n  "threadId": "t-own",\n  "codex": {\n    "status": 1,\n    "stdout": "%s"\n  }\n}\n' "$FAKE_GENERIC"
        exit 1
        ;;
    logged | logged-unthreaded)
        # Codex writes the real cause to its own log during the call. Another
        # session on the host logs a different error after it.
        thread=t-own
        [ "$(sed -n "${n}p" "$FAKE_PLAN")" = logged ] || thread=""
        sqlite3 "$HOME/.codex/logs_2.sqlite" \
            "insert into logs (ts, thread_id, feedback_log_body) values ($(date +%s), nullif('$thread', ''), 'turn sampling_error=$FAKE_FORBIDDEN');
             insert into logs (ts, thread_id, feedback_log_body) values ($(date +%s), 't-other', 'turn sampling_error=other 429 Too Many Requests');"
        printf '{\n  "threadId": "t-own",\n  "codex": {\n    "status": 1,\n    "stdout": "%s"\n  }\n}\n' "$FAKE_GENERIC"
        exit 1
        ;;
    *)
        echo "fake node: no plan for call $n" >&2
        exit 99
        ;;
esac
EOF
cat > "$TMP/bin/codex" << 'EOF'
#!/bin/bash
[ "${FAKE_CODEX_UP:-1}" = 1 ]
EOF
chmod +x "$TMP/bin/node" "$TMP/bin/codex"

export PATH="$TMP/bin:$PATH"
export FAKE_GENERIC="$GENERIC" FAKE_FORBIDDEN="$FORBIDDEN"
export HARDEN_REVIEW_PROBE_DELAY_S=0 HARDEN_REVIEW_RETRY_DELAY_S=0

# A fresh HOME with the companion installed, and a fresh reviewer plan.
setup() { # <name> <plan-line>...
    local name=$1
    shift
    export HOME="$TMP/$name"
    mkdir -p "$HOME/.claude/plugins/cache/openai-codex/codex/1.0.5/scripts"
    : > "$HOME/.claude/plugins/cache/openai-codex/codex/1.0.5/scripts/codex-companion.mjs"
    export FAKE_PLAN="$HOME/plan" FAKE_CALLS="$HOME/calls" FAKE_ARGS="$HOME/args"
    printf '%s\n' "$@" > "$FAKE_PLAN"
}

# Runs a Claude Code run's review and sets OUT (all output), LAST (its final
# line) and RC. Extra arguments reach the script.
review() {
    OUT=$("$CLI" --hardened-by claude-code "$@" 2>&1)
    RC=$?
    LAST=$(printf '%s\n' "$OUT" | tail -1)
}
calls() { cat "$FAKE_CALLS" 2>/dev/null || echo 0; }
# The upstream detail the status line quotes.
upstream() {
    local u=${LAST#*upstream: }
    printf "%s" "${u%)}"
}

echo "a reviewer that fails twice with a 403"
setup twice fail403 fail403
review
expect_eq "retried once" "2" "$(calls)"
expect_has "says there is no verdict" "Codex review: NO VERDICT after 2 attempts" "$LAST"
expect_eq "names the upstream status line" "ERROR: $FORBIDDEN" "$(upstream)"
expect_has "names the reviewer's own words" "$GENERIC" "$LAST"
expect_has "names the exit status" "exit 1" "$LAST"
expect_has "keeps the stderr of each attempt" "attempt 2 stderr" "$OUT"
expect_eq "exits 1" "1" "$RC"

echo "a reviewer that recovers on the retry"
setup recover fail403 ok
review
expect_eq "two attempts" "2" "$(calls)"
expect_eq "last line" "Codex review: verdict returned (attempt 2 of 2)" "$LAST"
expect_has "the verdict is in the output" "No actionable bugs found." "$OUT"
expect_eq "exits 0" "0" "$RC"

echo "a reviewer that answers first time"
setup first ok
review
expect_eq "one attempt" "1" "$(calls)"
expect_eq "last line" "Codex review: verdict returned (attempt 1 of 2)" "$LAST"
expect_lacks "no retry announced" "retrying" "$OUT"

echo "a zero exit that still says nothing"
setup silent silent0 silent0
review
expect_eq "retried once" "2" "$(calls)"
expect_has "no verdict" "Codex review: NO VERDICT after 2 attempts" "$LAST"
expect_has "exit 0 is named" "exit 0" "$LAST"

echo "a verdict that quotes the generic sentence"
setup quoting quotes
review
expect_eq "one attempt" "1" "$(calls)"
expect_eq "still a verdict" "Codex review: verdict returned (attempt 1 of 2)" "$LAST"

echo "a zero exit with an empty review"
setup empty empty empty
review
expect_eq "retried once" "2" "$(calls)"
expect_has "no verdict" "Codex review: NO VERDICT after 2 attempts" "$LAST"
expect_has "says it said nothing" "reviewer said \"nothing\"" "$LAST"

echo "a 403 only inside the JSON payload"
setup injson json403 json403
review
expect_has "names the upstream status" "$FORBIDDEN" "$(upstream)"

echo "no status anywhere"
setup bare bare bare
review
expect_has "says so" "upstream: none reported" "$LAST"

if command -v sqlite3 > /dev/null 2>&1; then
    echo "the Codex log holds the status"
    setup logged bare bare
    mkdir -p "$HOME/.codex"
    sqlite3 "$HOME/.codex/logs_2.sqlite" \
        "create table logs (id integer primary key autoincrement, ts integer not null, thread_id text, feedback_log_body text);
         insert into logs (ts, feedback_log_body) values ($(($(date +%s) - 86400)), 'turn sampling_error=stale 500 Internal Server Error');"
    review
    expect_has "a day-old error is not quoted" "upstream: none reported" "$LAST"
    printf 'logged\nlogged\n' > "$FAKE_PLAN"
    : > "$FAKE_CALLS"
    review
    expect_eq "its own thread's error is quoted" "sampling_error=$FORBIDDEN" "$(upstream)"
    printf 'logged-unthreaded\nlogged-unthreaded\n' > "$FAKE_PLAN"
    : > "$FAKE_CALLS"
    review
    expect_eq "an unthreaded error beats another thread's" "sampling_error=$FORBIDDEN" "$(upstream)"
    printf 'logged-multiline\nlogged-multiline\n' > "$FAKE_PLAN"
    : > "$FAKE_CALLS"
    review
    expect_eq "a multi-line error stays on the status line" "sampling_error=403 Forbidden at responses" "$(upstream)"
else
    fail "sqlite3 is not installed, so the Codex log cases cannot run"
fi

echo "the base reaches the reviewer"
setup base ok
review --base 0123abc
expect_eq "arguments" "$HOME/.claude/plugins/cache/openai-codex/codex/1.0.5/scripts/codex-companion.mjs review --scope branch --base 0123abc --json" "$(cat "$FAKE_ARGS")"

echo "the base defaults to main"
setup defaultbase ok
review
expect_has "arguments" "review --scope branch --base main --json" "$(cat "$FAKE_ARGS")"

echo "no plugin installed"
export HOME="$TMP/noplugin"
mkdir -p "$HOME"
review
expect_eq "last line" "Codex review: unavailable (plugin not installed)" "$LAST"
expect_eq "exits 2" "2" "$RC"

echo "a CLI that never answers"
setup down ok
FAKE_CODEX_UP=0 review
expect_eq "no review attempted" "0" "$(calls)"
expect_eq "last line" "Codex review: unavailable (CLI not answering after 3 probes)" "$LAST"
expect_eq "exits 2" "2" "$RC"

echo "an attempt that outlives its timeout"
setup hangs hang ok
HARDEN_REVIEW_ATTEMPT_TIMEOUT_S=1 review
expect_eq "not retried" "1" "$(calls)"
expect_has "no verdict, timed out" "Codex review: NO VERDICT after 1 attempt (timed out after 1s, reviewer said \"nothing\", upstream: reviewing)" "$LAST"
expect_eq "exits 1" "1" "$RC"

echo "the reviewer holds no Lucidos credential"
setup scrub ok
LUCIDOS_AGENT_ORIGIN_TOKEN=secret LUCIDOS_DATABASE_URL=postgres://x review
expect_eq "a verdict" "Codex review: verdict returned (attempt 1 of 2)" "$LAST"
expect_lacks "no origin token" "LUCIDOS_AGENT_ORIGIN_TOKEN=" "$(cat "$HOME/env")"
expect_lacks "no database URL" "LUCIDOS_DATABASE_URL=" "$(cat "$HOME/env")"

echo "an unknown backend"
setup unknown ok
OUT=$("$CLI" --hardened-by gemini 2>&1)
RC=$?
expect_eq "last line" "Other-model review: unavailable (unknown backend \"gemini\")" "$(printf '%s\n' "$OUT" | tail -1)"
expect_eq "exits 2" "2" "$RC"
expect_eq "no review attempted" "0" "$(calls)"
OUT=$("$CLI" --hardened-by "" 2>&1)
expect_eq "a missing backend is unknown too" "Other-model review: unavailable (unknown backend \"\")" "$OUT"

echo "decide: the other family reviews every full diff"
decide() { "$CLI" decide "$@"; }
for row in \
    "claude-code|run codex main|" \
    "codex|run claude main|" \
    "claude-code|run codex main|--scope FULL" \
    "codex|run claude main|--scope FULL" \
    "claude-code|run codex main|--scope INCREMENTAL" \
    "codex|run claude main|--scope INCREMENTAL" \
    "claude-code|run codex 0123abc|--scope INCREMENTAL --other-model-base 0123abc" \
    "codex|run claude 0123abc|--scope INCREMENTAL --other-model-base 0123abc" \
    "claude-code|Codex review: skipped (merge-only)|--scope MERGE_ONLY" \
    "codex|Claude review: skipped (merge-only)|--scope MERGE_ONLY" \
    "claude-code|Codex review: skipped (docs-only)|--docs-only" \
    "codex|Claude review: skipped (docs-only)|--docs-only" \
    "claude-code|Codex review: skipped (a fix of 60 lines, 60 or fewer)|--fix-lines 60" \
    "codex|Claude review: skipped (a fix of 12 lines, 60 or fewer)|--fix-lines 12" \
    "claude-code|run codex main|--fix-lines 61" \
    "codex|run claude 0123abc|--fix-lines 200 --other-model-base 0123abc"; do
    backend=${row%%|*} rest=${row#*|}
    want=${rest%%|*} facts=${rest#*|}
    # shellcheck disable=SC2086 # the facts are separate flags
    expect_eq "$backend $facts" "$want" "$(decide --hardened-by "$backend" $facts)"
done
expect_eq "decide: an unknown backend" "Other-model review: unavailable (unknown backend \"gemini\")" "$(decide --hardened-by gemini)"
decide --hardened-by gemini > /dev/null
expect_eq "decide: an unknown backend exits 2" "2" "$?"

# --- the Claude reviewer, for a Codex run ---

# The fake `claude`. `--version` answers by $FAKE_CLAUDE_UP; every other call
# takes the next line of $FAKE_PLAN, and records its argv, env, cwd and the
# diff it was handed.
cat > "$TMP/fake-claude" << 'EOF'
#!/bin/bash
if [ "${1:-}" = --version ]; then
    [ "${FAKE_CLAUDE_UP:-1}" = 1 ] && echo "9.9.9 (Claude Code)"
    exit $((1 - ${FAKE_CLAUDE_UP:-1}))
fi
n=$(($(cat "$FAKE_CALLS" 2>/dev/null || echo 0) + 1))
echo "$n" > "$FAKE_CALLS"
printf '%s\n' "$@" > "$FAKE_ARGS"
env > "$HOME/env"
pwd -P > "$HOME/cwd"
cp .lucidos/other-model-review.diff "$HOME/diff" 2>/dev/null
usage='"modelUsage": {"claude-opus-5-5": {"costUSD": 0.19}}, "total_cost_usd": 0.19'
case "$(sed -n "${n}p" "$FAKE_PLAN")" in
    findings)
        printf '{"type": "result", "subtype": "success", "is_error": false, "result": "Two findings.", %s, "structured_output": {"findings": [{"file": "src/a.rs", "line": 12, "category": "correctness", "summary": "The lock is never released.", "failure_scenario": "an early return\\nleaks it"}, {"file": "docs/b.md", "line": 3, "category": "cleanup", "summary": "Stale name.", "failure_scenario": "a reader follows it"}]}}\n' "$usage"
        ;;
    none)
        printf '{"type": "result", "subtype": "success", "is_error": false, "result": "", %s, "structured_output": {"findings": []}}\n' "$usage"
        ;;
    notsignedin)
        printf '{"type": "result", "subtype": "success", "is_error": true, "result": "Not logged in · Please run /login", "total_cost_usd": 0}\n'
        exit 1
        ;;
    forbidden)
        printf '{"type": "result", "subtype": "success", "is_error": true, "result": "API Error: 403 Forbidden {\\"error\\": {\\"message\\": \\"no access to model\\"}}", "total_cost_usd": 0}\n'
        exit 1
        ;;
    budget)
        printf '{"type": "result", "subtype": "error_max_budget_usd", "is_error": true, %s}\n' "$usage"
        exit 1
        ;;
    noschema)
        printf '{"type": "result", "subtype": "success", "is_error": false, "result": "The change looks fine to me.", %s}\n' "$usage"
        ;;
    hang)
        echo "thinking" >&2
        sleep 30
        ;;
    *)
        echo "fake claude: no plan for call $n" >&2
        exit 99
        ;;
esac
EOF
chmod +x "$TMP/fake-claude"

# A repository to review: a main branch and one commit beyond it.
git init -q -b main "$TMP/repo"
git -C "$TMP/repo" -c user.name=t -c user.email=t@example.com commit -q --allow-empty -m "base"
git -C "$TMP/repo" checkout -q -b topic
echo "changed" > "$TMP/repo/a.txt"
git -C "$TMP/repo" add a.txt
git -C "$TMP/repo" -c user.name=t -c user.email=t@example.com commit -q -m "the change under review"
MAIN_SHA=$(git -C "$TMP/repo" rev-parse main)
# No system `claude` is ever found: only the one a test puts in HOME.
export HARDEN_REVIEW_CLAUDE_DIRS="$TMP/no-system-claude"

# A fresh HOME with the fake `claude` installed, and a fresh reviewer plan.
setup_claude() { # <name> <plan-line>...
    setup "$@"
    mkdir -p "$HOME/.local/bin"
    cp "$TMP/fake-claude" "$HOME/.local/bin/claude"
}

# Runs a Codex run's review from inside the repository, like review().
claude_review() {
    OUT=$(cd "$TMP/repo" && "$CLI" --hardened-by codex "$@" 2>&1)
    RC=$?
    LAST=$(printf '%s\n' "$OUT" | tail -1)
}

# A PATH with the tools the script needs, and neither `claude` nor `jq`.
mkdir -p "$TMP/bare-bin"
for tool in dirname mktemp rm cat tail sleep date git grep sed awk tr cut find env; do
    ln -s "$(command -v "$tool")" "$TMP/bare-bin/$tool"
done

echo "claude: a verdict with findings"
setup_claude cfindings findings
claude_review
expect_eq "one attempt" "1" "$(calls)"
expect_eq "last line" "Claude review: verdict returned (attempt 1 of 2)" "$LAST"
expect_has "names the served model and cost" "Claude reviewer: claude-opus-5-5, \$0.19" "$OUT"
expect_has "one line per finding" "src/a.rs:12 [correctness] The lock is never released.: an early return leaks it" "$OUT"
expect_has "the second finding" "docs/b.md:3 [cleanup] Stale name.: a reader follows it" "$OUT"
expect_lacks "never the raw array" '"findings"' "$OUT"
expect_eq "exits 0" "0" "$RC"

echo "claude: a verdict with no findings"
setup_claude cnone none
claude_review
expect_eq "last line" "Claude review: verdict returned (attempt 1 of 2)" "$LAST"
expect_has "says so" "No findings." "$OUT"

echo "claude: a reviewer that recovers on the retry"
setup_claude crecover notsignedin findings
claude_review
expect_eq "two attempts" "2" "$(calls)"
expect_has "announces the retry" "Claude reviewer produced no verdict, retrying" "$OUT"
expect_eq "last line" "Claude review: verdict returned (attempt 2 of 2)" "$LAST"

echo "claude: not signed in"
setup_claude csignin notsignedin notsignedin
claude_review
expect_has "no verdict" "Claude review: NO VERDICT after 2 attempts (exit 1" "$LAST"
expect_has "quotes the reviewer" "reviewer said \"Not logged in · Please run /login\"" "$LAST"
expect_eq "names the error flag" "is_error, subtype success" "$(upstream)"
expect_eq "exits 1" "1" "$RC"

echo "claude: a 403 in the result"
setup_claude c403 forbidden forbidden
claude_review
expect_has "no verdict" "Claude review: NO VERDICT after 2 attempts" "$LAST"
expect_has "names the upstream status" "403 Forbidden" "$(upstream)"

echo "claude: an error with a subtype"
setup_claude cbudget budget budget
claude_review
expect_has "no verdict" "Claude review: NO VERDICT after 2 attempts" "$LAST"
expect_has "said nothing" "reviewer said \"nothing\"" "$LAST"
expect_eq "names the subtype" "is_error, subtype error_max_budget_usd" "$(upstream)"

echo "claude: no structured output"
setup_claude cschema noschema noschema
claude_review
expect_has "no verdict, exit 0" "Claude review: NO VERDICT after 2 attempts (exit 0" "$LAST"
expect_has "quotes the prose" "reviewer said \"The change looks fine to me.\"" "$LAST"
expect_eq "names the missing schema" "no structured_output, subtype success" "$(upstream)"

echo "claude: an attempt that outlives its timeout"
setup_claude chang hang findings
# Long enough for the git writes before `claude` starts, on a loaded host.
HARDEN_REVIEW_ATTEMPT_TIMEOUT_S=5 claude_review
expect_eq "not retried" "1" "$(calls)"
expect_eq "last line" "Claude review: NO VERDICT after 1 attempt (timed out after 5s, reviewer said \"nothing\", upstream: thinking)" "$LAST"
expect_eq "exits 1" "1" "$RC"

echo "claude: no CLI installed"
setup cmissing findings
PATH="$TMP/bare-bin" claude_review
expect_eq "last line" "Claude review: unavailable (CLI not installed)" "$LAST"
expect_eq "exits 2" "2" "$RC"

echo "claude: no jq installed"
setup_claude cnojq findings
PATH="$TMP/bare-bin" claude_review
expect_eq "no review attempted" "0" "$(calls)"
expect_eq "last line" "Claude review: unavailable (jq not installed)" "$LAST"

echo "claude: a CLI that never answers"
setup_claude cdown findings
FAKE_CLAUDE_UP=0 claude_review
expect_eq "no review attempted" "0" "$(calls)"
expect_has "probes three times" "probe 2 of 3" "$OUT"
expect_eq "last line" "Claude review: unavailable (CLI not answering after 3 probes)" "$LAST"
expect_eq "exits 2" "2" "$RC"

echo "claude: the call itself"
setup_claude cargv none
claude_review --base "$MAIN_SHA"
# A pinned literal: these flags are the reviewer's contract (plan decision 7).
schema=$(
    # shellcheck source=scripts/lib/other_model_review.sh
    . "$SCRIPT_DIR/other_model_review.sh"
    echo "$OMR_CLAUDE_FINDINGS_SCHEMA"
)
expect_eq "flags after the prompt" \
    "--output-format json --json-schema $schema --model opus --effort high --tools Read,Grep,Glob --permission-mode dontAsk --permission-prompts none --strict-mcp-config --no-session-persistence --max-budget-usd 5" \
    "$(sed -n '3,$p' "$FAKE_ARGS" | tr '\n' ' ' | sed 's/ $//')"
expect_eq "print mode first" "-p" "$(sed -n 1p "$FAKE_ARGS")"
expect_has "the prompt names the base" "$MAIN_SHA...HEAD" "$(sed -n 2p "$FAKE_ARGS")"
expect_has "the prompt points at the skill" ".claude/skills/code-review/SKILL.md" "$(sed -n 2p "$FAKE_ARGS")"
expect_eq "runs at the repository root" "$TMP/repo" "$(cat "$HOME/cwd")"
expect_has "the diff holds the commit log" "the change under review" "$(cat "$HOME/diff")"
expect_has "the diff holds the change" "+changed" "$(cat "$HOME/diff")"

echo "claude: the base defaults to main"
setup_claude cdefault none
claude_review
expect_has "the prompt names main" "main...HEAD" "$(sed -n 2p "$FAKE_ARGS")"

echo "claude: the env scrub, and CLAUDE_CONFIG_DIR exactly as given"
setup_claude cenvset none
CLAUDE_CONFIG_DIR="$TMP/a config" LUCIDOS_AGENT_ORIGIN_TOKEN=secret LUCIDOS_DATABASE_URL=postgres://x claude_review
expect_eq "a verdict" "Claude review: verdict returned (attempt 1 of 2)" "$LAST"
expect_lacks "no origin token" "LUCIDOS_AGENT_ORIGIN_TOKEN=" "$(cat "$HOME/env")"
expect_lacks "no database URL" "LUCIDOS_DATABASE_URL=" "$(cat "$HOME/env")"
expect_has "a set config dir passes through" "CLAUDE_CONFIG_DIR=$TMP/a config" "$(cat "$HOME/env")"
setup_claude cenvunset none
unset CLAUDE_CONFIG_DIR
claude_review
expect_lacks "an unset config dir stays unset" "CLAUDE_CONFIG_DIR=" "$(cat "$HOME/env")"

echo "the watchdog keeps the reviewer in the task's process group"
# The engine's stop kills the task's process group, so a reviewer in a group
# of its own would keep spending. The watchdog's own timeout kills the tree.
(
    # shellcheck source=scripts/lib/other_model_review.sh
    . "$SCRIPT_DIR/other_model_review.sh"
    # shellcheck disable=SC2016 # the inner bash expands $$, $! and $1
    omr_with_timeout 2 bash -c 'ps -o pgid= -p $$ | tr -d " " > "$1/pgid"; sleep 30 & echo $! > "$1/grandchild"; wait' _ "$TMP"
    echo $? > "$TMP/watchdog-rc"
)
expect_eq "same process group" "$(ps -o pgid= -p $$ | tr -d ' ')" "$(cat "$TMP/pgid")"
expect_eq "reports the timeout" "124" "$(cat "$TMP/watchdog-rc")"
grandchild=$(cat "$TMP/grandchild")
for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$grandchild" 2> /dev/null || break; sleep 0.2; done
if kill -0 "$grandchild" 2> /dev/null; then
    fail "the reviewer's child outlived the timeout"
    kill "$grandchild"
else
    pass "the reviewer's child died with it"
fi

echo "the task timer outlasts every attempt"
budget=$(
    unset HARDEN_REVIEW_PROBE_DELAY_S HARDEN_REVIEW_RETRY_DELAY_S HARDEN_REVIEW_ATTEMPT_TIMEOUT_S
    # shellcheck source=scripts/lib/other_model_review.sh
    . "$SCRIPT_DIR/other_model_review.sh"
    echo $((OMR_ATTEMPTS * OMR_ATTEMPT_TIMEOUT_S + 2 * OMR_PROBE_DELAY_S + OMR_RETRY_DELAY_S))
)
timer=$("$CLI" task-timeout)
if [ "$timer" -gt "$budget" ]; then pass "task timer $timer > budget $budget"; else fail "task timer $timer <= budget $budget"; fi

echo
echo "harden_other_model_review: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
