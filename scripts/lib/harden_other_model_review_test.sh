#!/bin/bash
# Tests for scripts/harden-other-model-review.sh: the other-model review
# `/harden` launches in Phase 1 and joins in Phase 3.
#
# Hermetic: HOME is a temp dir, and a fake `codex` leads PATH. A fake `claude`
# sits in each Claude case's HOME, and no system `claude` is searched. Each
# fake plays one scripted outcome per attempt. Nothing reaches a real CLI, its
# logs, or the network. The suite clears the inherited agent-sandbox marker,
# so the host's own sandbox never decides a case.
#
# A verdict counts only with proof the reviewer read the patch (ADR 0558,
# amended). Both reviewers get cases for a real verdict, with and without
# findings, and for every shape that must NOT count: a sandbox error, "unable
# to assess" in several wordings, `status: unable`, no files, the wrong files,
# empty output and garbage. Codex also covers its sandbox probe (applies,
# refused under the Lucidos sandbox, refused with none), the snapshot it
# reviews, the upstream causes, the retry and the timeout. Claude also covers
# its pinned argv, the diff it reads and CLAUDE_CONFIG_DIR. Shared: the
# `decide` table, an unknown backend, a bad base, the env scrub, the
# watchdog's process group and the task timer.
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

# The host's sandbox state must not leak into a case: each sets it itself.
unset LUCIDOS_AGENT_SANDBOXED

FORBIDDEN="unexpected status 403 Forbidden: Project does not have access to model gpt-6-astra"
SANDBOX_ERR="sandbox-exec: sandbox_apply: Operation not permitted"

# A repository to review: a main branch and one commit beyond it, which
# changes a.txt. The Claude reviewer writes its diff under .lucidos/.
git init -q -b main "$TMP/repo"
git -C "$TMP/repo" -c user.name=t -c user.email=t@example.com commit -q --allow-empty -m "base"
git -C "$TMP/repo" checkout -q -b topic
echo "changed" > "$TMP/repo/a.txt"
git -C "$TMP/repo" add a.txt
git -C "$TMP/repo" -c user.name=t -c user.email=t@example.com commit -q -m "the change under review"
echo ".lucidos/" >> "$TMP/repo/.git/info/exclude"
MAIN_SHA=$(git -C "$TMP/repo" rev-parse main)
HEAD_SHA=$(git -C "$TMP/repo" rev-parse HEAD)

# A second repository whose change renames old.txt to new.txt.
git init -q -b main "$TMP/renamed"
echo "same" > "$TMP/renamed/old.txt"
git -C "$TMP/renamed" add old.txt
git -C "$TMP/renamed" -c user.name=t -c user.email=t@example.com commit -q -m "base"
git -C "$TMP/renamed" checkout -q -b topic
git -C "$TMP/renamed" mv old.txt new.txt
git -C "$TMP/renamed" -c user.name=t -c user.email=t@example.com commit -q -m "a rename"
echo ".lucidos/" >> "$TMP/renamed/.git/info/exclude"

# A jq that fails the file-list comparison and passes every other call.
mkdir -p "$TMP/broken-jq"
cat > "$TMP/broken-jq/jq" << EOF
#!/bin/bash
case " \$* " in *" --rawfile "*) echo "jq: error: broken" >&2; exit 2 ;; esac
exec $(command -v jq) "\$@"
EOF
chmod +x "$TMP/broken-jq/jq"

mkdir -p "$TMP/bin"
# The fake Codex CLI. Readiness calls answer by $FAKE_CODEX_UP, the sandbox
# probe by $FAKE_CODEX_SANDBOX. Each `exec` takes the next line of $FAKE_PLAN
# as its outcome, and records its argv, env, cwd, prompt, schema and HEAD.
cat > "$TMP/bin/codex" << 'EOF'
#!/bin/bash
up=${FAKE_CODEX_UP:-1}
case "${1:-}" in
    --version)
        [ "$up" = 1 ]
        exit
        ;;
    sandbox)
        printf '%s\n' "$*" >> "$HOME/probes"
        [ "${FAKE_CODEX_SANDBOX:-applies}" = applies ] && exit 0
        echo "$FAKE_SANDBOX_ERR" >&2
        exit 71
        ;;
    exec)
        if [ "${2:-}" = --help ]; then
            [ "$up" = 1 ]
            exit
        fi
        ;;
    *)
        echo "fake codex: unexpected call: $*" >&2
        exit 98
        ;;
esac
n=$(($(cat "$FAKE_CALLS" 2>/dev/null || echo 0) + 1))
echo "$n" > "$FAKE_CALLS"
printf '%s\n' "$@" > "$FAKE_ARGS"
env > "$HOME/env"
cat > "$HOME/prompt"
tree="" prev=""
for arg in "$@"; do
    [ "$prev" = -C ] && tree=$arg
    [ "$prev" = --output-schema ] && cp "$arg" "$HOME/schema"
    prev=$arg
done
cd "$tree" || exit 97
pwd -P > "$HOME/cwd"
git rev-parse HEAD > "$HOME/head"

event() { jq -n -c "$@"; }
ran() { # <command> <exit> <output>
    event --arg c "$1" --argjson e "$2" --arg o "$3" \
        '{type: "item.completed", item: {type: "command_execution", command: $c, exit_code: $e, aggregated_output: $o, status: "completed"}}'
}
says() { # <text>
    event --arg t "$1" '{type: "item.completed", item: {type: "agent_message", text: $t}}'
}
verdict() { # <status> <changed files JSON> <findings JSON> [summary]
    says "$(jq -n -c --arg s "$1" --argjson c "$2" --argjson f "$3" --arg m "${4:-Read the whole diff.}" \
        '{status: $s, summary: $m, changed_files: $c, findings: $f}')"
}
started() {
    event '{type: "thread.started", thread_id: "t-own"}'
    event '{type: "turn.started"}'
}
completed() { event '{type: "turn.completed", usage: {}}'; }
read_diff() { ran "/bin/zsh -lc 'git diff'" 0 "+changed"; }
log_error() { # <thread or ""> <body>
    sqlite3 "$HOME/.codex/logs_2.sqlite" \
        "insert into logs (ts, thread_id, feedback_log_body) values ($(date +%s), nullif('$1', ''), '$2');"
}
case "$(sed -n "${n}p" "$FAKE_PLAN")" in
    ok)
        started
        read_diff
        verdict reviewed '["a.txt"]' '[]'
        completed
        ;;
    findings)
        started
        read_diff
        verdict reviewed '["./a.txt"]' '[{"file": "a.txt", "line": 1, "category": "correctness", "summary": "The value is wrong.", "failure_scenario": "a reader\ngets it"}]'
        completed
        ;;
    renamed)
        started
        read_diff
        verdict reviewed '["old.txt", "new.txt"]' '[]'
        completed
        ;;
    writes)
        echo "x" > reviewer-was-here
        git add reviewer-was-here
        git -c user.name=r -c user.email=r@example.com commit -q -m "a reviewer commit"
        started
        read_diff
        verdict reviewed '["a.txt"]' '[]'
        completed
        ;;
    sandboxed)
        started
        ran "/bin/zsh -lc 'git diff'" 71 "$FAKE_SANDBOX_ERR"
        says "Unable to assess the patch: running the requested git diff failed with \`$FAKE_SANDBOX_ERR\`. The verdict is not a confirmed code defect."
        completed
        ;;
    unverified)
        started
        says "Unable to verify this verdict: the requested git diff failed. No code was inspected."
        completed
        ;;
    unable)
        started
        verdict unable '[]' '[]' "Unable to assess the patch: the diff could not be read."
        completed
        ;;
    nofiles)
        started
        verdict reviewed '[]' '[]'
        completed
        ;;
    wrongfiles)
        started
        read_diff
        verdict reviewed '["b.txt"]' '[]'
        completed
        ;;
    prose)
        started
        read_diff
        says "The change looks fine to me."
        completed
        ;;
    empty) ;;
    garbage)
        echo "%% not an event stream %%"
        ;;
    fail403)
        started
        event --arg m "$FAKE_FORBIDDEN" '{type: "turn.failed", error: {message: $m}}'
        echo "ERROR: $FAKE_FORBIDDEN" >&2
        exit 1
        ;;
    event403)
        started
        event --arg m "$FAKE_FORBIDDEN" '{type: "error", message: $m}'
        exit 1
        ;;
    bare)
        started
        exit 1
        ;;
    logged)
        # Codex writes the real cause to its own log during the call. Another
        # session on the host logs a different error after it.
        log_error t-own "turn sampling_error=$FAKE_FORBIDDEN"
        log_error t-other "turn sampling_error=other 429 Too Many Requests"
        started
        exit 1
        ;;
    logged-unthreaded)
        log_error "" "turn sampling_error=$FAKE_FORBIDDEN"
        log_error t-other "turn sampling_error=other 429 Too Many Requests"
        started
        exit 1
        ;;
    logged-multiline)
        sqlite3 "$HOME/.codex/logs_2.sqlite" \
            "insert into logs (ts, thread_id, feedback_log_body) values ($(date +%s), 't-own', 'turn sampling_error=403 Forbidden' || char(10) || 'at responses');"
        started
        exit 1
        ;;
    hang)
        echo "reviewing" >&2
        sleep 30
        ;;
    *)
        echo "fake codex: no plan for call $n" >&2
        exit 99
        ;;
esac
EOF
chmod +x "$TMP/bin/codex"

export PATH="$TMP/bin:$PATH"
export FAKE_FORBIDDEN="$FORBIDDEN" FAKE_SANDBOX_ERR="$SANDBOX_ERR"
export HARDEN_REVIEW_PROBE_DELAY_S=0 HARDEN_REVIEW_RETRY_DELAY_S=0

# A fresh HOME and a fresh reviewer plan.
setup() { # <name> <plan-line>...
    local name=$1
    shift
    export HOME="$TMP/$name"
    mkdir -p "$HOME"
    export FAKE_PLAN="$HOME/plan" FAKE_CALLS="$HOME/calls" FAKE_ARGS="$HOME/args"
    printf '%s\n' "$@" > "$FAKE_PLAN"
}

# Runs a review from inside $REVIEW_REPO and sets OUT (all output), LAST (its
# final line) and RC. Extra arguments reach the script.
REVIEW_REPO="$TMP/repo"
run_review() { # <hardened-by> [arg]...
    OUT=$(cd "$REVIEW_REPO" && "$CLI" --hardened-by "$@" 2>&1)
    RC=$?
    LAST=$(printf '%s\n' "$OUT" | tail -1)
}
# A Claude Code run's review, by Codex.
review() { run_review claude-code "$@"; }
calls() { cat "$FAKE_CALLS" 2>/dev/null || echo 0; }
# The upstream detail the status line quotes.
upstream() {
    local u=${LAST#*upstream: }
    printf "%s" "${u%)}"
}

# Asserts that the review ran twice and ended with no verdict, exit 1.
expect_no_verdict() { # <label> <reason>
    expect_eq "$1: retried once" "2" "$(calls)"
    expect_has "$1: no verdict" "NO VERDICT after 2 attempts (exit 0, $2" "$LAST"
    expect_lacks "$1: never a verdict" "verdict returned" "$OUT"
    expect_eq "$1: exits 1" "1" "$RC"
}

echo "codex: a verdict with no findings"
setup first ok
review
expect_eq "one attempt" "1" "$(calls)"
expect_eq "last line" "Codex review: verdict returned (attempt 1 of 2)" "$LAST"
expect_has "the summary is in the output" "summary: Read the whole diff." "$OUT"
expect_has "says so" "No findings." "$OUT"
expect_has "lists what the reviewer ran" "ran: /bin/zsh -lc 'git diff' (exit 0)" "$OUT"
expect_lacks "no retry announced" "retrying" "$OUT"
expect_eq "exits 0" "0" "$RC"

echo "codex: a verdict with findings"
setup cxfindings findings
review
expect_eq "last line" "Codex review: verdict returned (attempt 1 of 2)" "$LAST"
expect_has "one line per finding" "a.txt:1 [correctness] The value is wrong.: a reader gets it" "$OUT"
expect_lacks "never the raw array" '"findings"' "$OUT"

echo "codex: the sandbox_apply error, told as prose"
setup cxsandboxed sandboxed sandboxed
review
expect_no_verdict "sandbox error" "no structured verdict"
expect_has "quotes the reviewer" "reviewer said \"Unable to assess the patch: running the requested git diff failed" "$LAST"
expect_eq "names the failed command" "a command failed (exit 71): $SANDBOX_ERR" "$(upstream)"

echo "codex: another unable-to-assess wording"
setup cxunverified unverified unverified
review
expect_no_verdict "unverified" "no structured verdict"
expect_has "quotes the reviewer" "Unable to verify this verdict" "$LAST"

echo "codex: a structured unable"
setup cxunable unable unable
review
expect_no_verdict "unable" "the reviewer said it could not review"
expect_has "quotes its summary" "reviewer said \"Unable to assess the patch: the diff could not be read.\"" "$LAST"

echo "codex: reviewed, but names no files"
setup cxnofiles nofiles nofiles
review
expect_no_verdict "no files" "its changed files do not match the diff: missing a.txt"

echo "codex: reviewed, but names the wrong files"
setup cxwrong wrongfiles wrongfiles
review
expect_no_verdict "wrong files" "its changed files do not match the diff: missing a.txt, unexpected b.txt"

echo "codex: a rename names both paths"
setup cxrenamed renamed
REVIEW_REPO="$TMP/renamed" review
expect_eq "last line" "Codex review: verdict returned (attempt 1 of 2)" "$LAST"

echo "codex: a comparison that fails closes the proof"
setup cxbrokenjq ok ok
PATH="$TMP/broken-jq:$PATH" review
expect_no_verdict "broken jq" "could not compare its changed files with the diff"

echo "codex: an unstructured answer"
setup cxprose prose prose
review
expect_no_verdict "prose" "no structured verdict"
expect_has "quotes the prose" "reviewer said \"The change looks fine to me.\"" "$LAST"

echo "codex: empty output"
setup cxempty empty empty
review
expect_no_verdict "empty" "no structured verdict"
expect_has "says it said nothing" "reviewer said \"nothing\"" "$LAST"

echo "codex: garbage"
setup cxgarbage garbage garbage
review
expect_no_verdict "garbage" "no structured verdict"
expect_has "shows the garbage" "%% not an event stream %%" "$OUT"

echo "codex: a reviewer that fails twice with a 403"
setup twice fail403 fail403
review
expect_eq "retried once" "2" "$(calls)"
expect_has "says there is no verdict" "Codex review: NO VERDICT after 2 attempts (exit 1," "$LAST"
expect_eq "names the upstream status line" "ERROR: $FORBIDDEN" "$(upstream)"
expect_has "keeps the stderr of each attempt" "attempt 2 stderr" "$OUT"
expect_has "shows the failed turn" "turn failed: $FORBIDDEN" "$OUT"
expect_eq "exits 1" "1" "$RC"

echo "codex: a reviewer that recovers on the retry"
setup recover fail403 ok
review
expect_eq "two attempts" "2" "$(calls)"
expect_eq "last line" "Codex review: verdict returned (attempt 2 of 2)" "$LAST"
expect_eq "exits 0" "0" "$RC"

echo "codex: a 403 only in an error event"
setup inevent event403 event403
review
expect_has "names the upstream status" "$FORBIDDEN" "$(upstream)"

echo "codex: no status anywhere"
setup bare bare bare
review
expect_has "says so" "upstream: none reported" "$LAST"

if command -v sqlite3 > /dev/null 2>&1; then
    echo "codex: the Codex log holds the status"
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

echo "codex: its own sandbox applies, so it stays read-only"
setup cxreadonly ok
review
expect_eq "probed Codex's sandbox" "sandbox -- true" "$(cat "$HOME/probes")"
expect_has "read-only" "$(printf -- '--sandbox\nread-only')" "$(cat "$FAKE_ARGS")"
expect_has "says so" "CODEX_SANDBOX=read-only" "$OUT"
setup cxreadonlymarked ok
LUCIDOS_AGENT_SANDBOXED=1 review
expect_has "read-only even under the marker" "$(printf -- '--sandbox\nread-only')" "$(cat "$FAKE_ARGS")"

echo "codex: its sandbox cannot nest in the Lucidos sandbox"
setup cxnested ok
FAKE_CODEX_SANDBOX=nested LUCIDOS_AGENT_SANDBOXED=1 review
expect_eq "a verdict" "Codex review: verdict returned (attempt 1 of 2)" "$LAST"
expect_has "no nested sandbox" "$(printf -- '--sandbox\ndanger-full-access')" "$(cat "$FAKE_ARGS")"
expect_lacks "never read-only" "read-only" "$(cat "$FAKE_ARGS")"
expect_has "says why" "CODEX_SANDBOX=danger-full-access (Codex's own sandbox cannot nest in the Lucidos agent sandbox, which bounds this review" "$OUT"

echo "codex: its sandbox cannot run, and no Lucidos sandbox wraps it"
setup cxunbounded ok
FAKE_CODEX_SANDBOX=nested review
expect_eq "no review attempted" "0" "$(calls)"
expect_eq "last line" "Codex review: unavailable (Codex's sandbox cannot run here and no Lucidos sandbox wraps this review: $SANDBOX_ERR)" "$LAST"
expect_eq "exits 2" "2" "$RC"

echo "codex: the call itself"
setup cxargv ok
review --base "$MAIN_SHA"
expect_eq "exec first" "exec" "$(sed -n 1p "$FAKE_ARGS")"
expect_has "no approvals to answer" "$(printf -- '-c\napproval_policy="never"')" "$(cat "$FAKE_ARGS")"
expect_has "ephemeral, events as JSON" "$(printf -- '--ephemeral\n--json\n--output-schema')" "$(cat "$FAKE_ARGS")"
expect_eq "the prompt on stdin" "-" "$(tail -1 "$FAKE_ARGS")"
schema=$(
    # shellcheck source=scripts/lib/other_model_review.sh
    . "$SCRIPT_DIR/other_model_review.sh"
    echo "$OMR_FINDINGS_SCHEMA"
)
expect_eq "the shared schema" "$schema" "$(cat "$HOME/schema")"
expect_has "the prompt names the frozen range" "$MAIN_SHA...$HEAD_SHA" "$(cat "$HOME/prompt")"
expect_has "the prompt points at the skill" ".claude/skills/code-review/SKILL.md" "$(cat "$HOME/prompt")"
expect_has "the prompt asks for the proof" "changed_files" "$(cat "$HOME/prompt")"
expect_has "the prompt lists files as the proof check does" \
    "git -c core.quotePath=false diff --no-renames --name-only $MAIN_SHA...$HEAD_SHA" "$(cat "$HOME/prompt")"

echo "codex: the base defaults to main"
setup cxdefault ok
review
expect_has "the prompt names main's commit" "$MAIN_SHA...$HEAD_SHA" "$(cat "$HOME/prompt")"

echo "codex: the reviewer works on a snapshot"
setup cxwrites writes
review
expect_eq "a verdict" "Codex review: verdict returned (attempt 1 of 2)" "$LAST"
expect_eq "it reviewed the frozen head" "$HEAD_SHA" "$(cat "$HOME/head")"
expect_lacks "not in the live repository" "$TMP/repo" "$(cat "$HOME/cwd")"
expect_eq "the branch keeps its head" "$HEAD_SHA" "$(git -C "$TMP/repo" rev-parse HEAD)"
expect_eq "the live tree is clean" "" "$(git -C "$TMP/repo" status --porcelain)"

echo "codex: no CLI installed"
# A PATH with the tools the script needs, and neither `codex`, `claude` nor `jq`.
mkdir -p "$TMP/bare-bin"
for tool in dirname mktemp rm cat tail head sleep date git grep sed awk tr cut find env; do
    ln -s "$(command -v "$tool")" "$TMP/bare-bin/$tool"
done
setup cxmissing ok
PATH="$TMP/bare-bin" review
expect_eq "last line" "Codex review: unavailable (CLI not installed)" "$LAST"
expect_eq "exits 2" "2" "$RC"

echo "codex: no jq installed"
mkdir -p "$TMP/nojq-bin"
ln -s "$TMP/bin/codex" "$TMP/nojq-bin/codex"
setup cxnojq ok
PATH="$TMP/nojq-bin:$TMP/bare-bin" review
expect_eq "no review attempted" "0" "$(calls)"
expect_eq "last line" "Codex review: unavailable (jq not installed)" "$LAST"

echo "codex: a CLI that never answers"
setup down ok
FAKE_CODEX_UP=0 review
expect_eq "no review attempted" "0" "$(calls)"
expect_eq "last line" "Codex review: unavailable (CLI not answering after 3 probes)" "$LAST"
expect_eq "exits 2" "2" "$RC"

echo "codex: an attempt that outlives its timeout"
setup hangs hang ok
# Long enough for the snapshot before `codex` starts, on a loaded host.
HARDEN_REVIEW_ATTEMPT_TIMEOUT_S=5 review
expect_eq "not retried" "1" "$(calls)"
expect_eq "last line" "Codex review: NO VERDICT after 1 attempt (timed out after 5s, reviewer said \"nothing\", upstream: reviewing)" "$LAST"
expect_eq "exits 1" "1" "$RC"

echo "codex: the reviewer holds no Lucidos credential"
setup scrub ok
LUCIDOS_AGENT_ORIGIN_TOKEN=secret LUCIDOS_DATABASE_URL=postgres://x review
expect_eq "a verdict" "Codex review: verdict returned (attempt 1 of 2)" "$LAST"
expect_lacks "no origin token" "LUCIDOS_AGENT_ORIGIN_TOKEN=" "$(cat "$HOME/env")"
expect_lacks "no database URL" "LUCIDOS_DATABASE_URL=" "$(cat "$HOME/env")"

echo "a base that is not a commit"
setup badbase ok
review --base no-such-ref
expect_eq "no review attempted" "0" "$(calls)"
expect_eq "last line" "Codex review: unavailable (base \"no-such-ref\" is not a commit)" "$LAST"
expect_eq "exits 2" "2" "$RC"

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
# A result whose structured output is <structured JSON>.
result() { # <result text> <structured JSON>
    printf '{"type": "result", "subtype": "success", "is_error": false, "result": "%s", %s, "structured_output": %s}\n' "$1" "$usage" "$2"
}
case "$(sed -n "${n}p" "$FAKE_PLAN")" in
    findings)
        result "Two findings." '{"status": "reviewed", "summary": "Read the whole diff.", "changed_files": ["a.txt"], "findings": [{"file": "src/a.rs", "line": 12, "category": "correctness", "summary": "The lock is never released.", "failure_scenario": "an early return\nleaks it"}, {"file": "docs/b.md", "line": 3, "category": "cleanup", "summary": "Stale name.", "failure_scenario": "a reader follows it"}]}'
        ;;
    none)
        result "" '{"status": "reviewed", "summary": "Read the whole diff.", "changed_files": ["a.txt"], "findings": []}'
        ;;
    renamed)
        result "" '{"status": "reviewed", "summary": "Read the whole diff.", "changed_files": ["old.txt", "new.txt"], "findings": []}'
        ;;
    unable)
        result "" '{"status": "unable", "summary": "Unable to assess the patch: the diff file was empty.", "changed_files": [], "findings": []}'
        ;;
    nofiles)
        result "" '{"status": "reviewed", "summary": "Looks fine.", "changed_files": [], "findings": []}'
        ;;
    wrongfiles)
        result "" '{"status": "reviewed", "summary": "Looks fine.", "changed_files": ["b.txt"], "findings": []}'
        ;;
    legacy)
        result "Unable to assess the patch." '{"findings": []}'
        ;;
    garbage)
        echo "%% not a result %%"
        ;;
    empty) ;;
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

# No system `claude` is ever found: only the one a test puts in HOME.
export HARDEN_REVIEW_CLAUDE_DIRS="$TMP/no-system-claude"

# A fresh HOME with the fake `claude` installed, and a fresh reviewer plan.
setup_claude() { # <name> <plan-line>...
    setup "$@"
    mkdir -p "$HOME/.local/bin"
    cp "$TMP/fake-claude" "$HOME/.local/bin/claude"
}

# A Codex run's review, by Claude.
claude_review() { run_review codex "$@"; }

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

echo "claude: a rename names both paths"
setup_claude crenamed renamed
REVIEW_REPO="$TMP/renamed" claude_review
expect_eq "last line" "Claude review: verdict returned (attempt 1 of 2)" "$LAST"
expect_has "the diff shows the old path go" "deleted file mode" "$(cat "$HOME/diff")"
expect_lacks "the diff pairs no rename" "rename from" "$(cat "$HOME/diff")"

echo "claude: a verdict with no findings"
setup_claude cnone none
claude_review
expect_eq "last line" "Claude review: verdict returned (attempt 1 of 2)" "$LAST"
expect_has "says so" "No findings." "$OUT"

echo "claude: a structured unable"
setup_claude cunable unable unable
claude_review
expect_no_verdict "claude unable" "the reviewer said it could not review"
expect_has "quotes its summary" "reviewer said \"Unable to assess the patch: the diff file was empty.\"" "$LAST"

echo "claude: reviewed, but names no files"
setup_claude cnofiles nofiles nofiles
claude_review
expect_no_verdict "claude no files" "its changed files do not match the diff: missing a.txt"

echo "claude: reviewed, but names the wrong files"
setup_claude cwrong wrongfiles wrongfiles
claude_review
expect_no_verdict "claude wrong files" "its changed files do not match the diff: missing a.txt, unexpected b.txt"

echo "claude: an empty findings list with no proof"
setup_claude clegacy legacy legacy
claude_review
expect_no_verdict "claude legacy" "no structured verdict"

echo "claude: garbage"
setup_claude cgarbage garbage garbage
claude_review
expect_no_verdict "claude garbage" "no structured verdict"
expect_has "quotes the garbage" "reviewer said \"%% not a result %%\"" "$LAST"

echo "claude: empty output"
setup_claude cempty empty empty
claude_review
expect_no_verdict "claude empty" "no structured verdict"
expect_has "said nothing" "reviewer said \"nothing\"" "$LAST"

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
expect_no_verdict "claude no schema" "no structured verdict"
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
expect_eq "flags after the prompt" \
    "--output-format json --json-schema $schema --model opus --effort high --tools Read,Grep,Glob --permission-mode dontAsk --permission-prompts none --strict-mcp-config --no-session-persistence --max-budget-usd 5" \
    "$(sed -n '3,$p' "$FAKE_ARGS" | tr '\n' ' ' | sed 's/ $//')"
expect_eq "print mode first" "-p" "$(sed -n 1p "$FAKE_ARGS")"
expect_has "the prompt names the frozen range" "$MAIN_SHA...$HEAD_SHA" "$(sed -n 2p "$FAKE_ARGS")"
expect_has "the prompt points at the skill" ".claude/skills/code-review/SKILL.md" "$(sed -n 2p "$FAKE_ARGS")"
expect_has "the prompt asks for the proof" "changed_files" "$(sed -n 2p "$FAKE_ARGS")"
expect_has "the prompt lists files as the proof check does" \
    "git -c core.quotePath=false diff --no-renames --name-only $MAIN_SHA...$HEAD_SHA" "$(sed -n 2p "$FAKE_ARGS")"
expect_eq "runs at the repository root" "$TMP/repo" "$(cat "$HOME/cwd")"
expect_has "the diff holds the commit log" "the change under review" "$(cat "$HOME/diff")"
expect_has "the diff holds the change" "+changed" "$(cat "$HOME/diff")"

echo "claude: the base defaults to main"
setup_claude cdefault none
claude_review
expect_has "the prompt names main's commit" "$MAIN_SHA...$HEAD_SHA" "$(sed -n 2p "$FAKE_ARGS")"

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
