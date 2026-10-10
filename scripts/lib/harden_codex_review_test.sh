#!/bin/bash
# Tests for scripts/harden-codex-review.sh: the Codex reviewer `/harden`
# launches in Phase 1 and joins in Phase 3.
#
# Hermetic: HOME is a temp dir, and fake `node` and `codex` binaries lead
# PATH. The fake `node` plays one scripted reviewer outcome per attempt.
# Nothing reaches the real Codex CLI, its logs, or the network.
#
# Covered: a reviewer that fails twice with a 403, one that recovers on the
# retry, one that answers first time, a zero exit that still says nothing, a
# 403 only inside the JSON payload, the Codex log as the status source (fresh
# and stale), the base passed through, no plugin, and a CLI that never answers.
#
# Run: ./scripts/lib/harden_codex_review_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLI="$SCRIPT_DIR/../harden-codex-review.sh"

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
        printf '{\n  "codex": {\n    "status": 0,\n    "stdout": "No actionable bugs found."\n  }\n}\n'
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
export HARDEN_CODEX_PROBE_DELAY_S=0 HARDEN_CODEX_RETRY_DELAY_S=0

# A fresh HOME with the companion installed, and a fresh reviewer plan.
setup() { # <name> <plan-line>...
    local name=$1
    shift
    export HOME="$TMP/$name"
    mkdir -p "$HOME/.claude/plugins/cache/openai-codex/codex/1.0.5/scripts"
    : > "$HOME/.claude/plugins/cache/openai-codex/codex/1.0.5/scripts/codex-companion.mjs"
    export FAKE_PLAN="$HOME/plan" FAKE_CALLS="$HOME/calls" FAKE_ARGS="$HOME/args"
    printf '%s\n' "$@" > "$FAKE_PLAN"
    unset CODEX_BASE
}

# Runs the reviewer and sets OUT (all output), LAST (its final line) and RC.
review() {
    OUT=$("$CLI" 2>&1)
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
CODEX_BASE=0123abc review
expect_eq "arguments" "$HOME/.claude/plugins/cache/openai-codex/codex/1.0.5/scripts/codex-companion.mjs review --scope branch --base 0123abc --json" "$(cat "$FAKE_ARGS")"

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

echo
echo "harden_codex_review: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
