#!/bin/bash
# The reviewer adapters and shared helpers behind scripts/harden-other-model-review.sh.
#
# Each reviewer <r> is a set of functions, called by the entry script:
#
#   omr_<r>_ready              probe the reviewer; on failure set OMR_WHY, return 1
#   omr_<r>_run <base>         review <base>...HEAD; stdout and stderr are files
#   omr_<r>_show <out>         print the attempt's review for the .out record
#   omr_<r>_verdict <exit> <out>  return 0 when the attempt holds a verdict
#   omr_<r>_said <out>         what the reviewer itself said, one line
#   omr_<r>_upstream <out> <err> <started>  the upstream cause, one line
#
# Tested by scripts/lib/harden_other_model_review_test.sh.

# shellcheck disable=SC2034 # read by scripts/harden-other-model-review.sh, which sources this
OMR_ATTEMPTS=2
# shellcheck disable=SC2034 # read by scripts/harden-other-model-review.sh, which sources this
OMR_ATTEMPT_TIMEOUT_S=${HARDEN_REVIEW_ATTEMPT_TIMEOUT_S:-900}
OMR_PROBE_DELAY_S=${HARDEN_REVIEW_PROBE_DELAY_S:-30}
# shellcheck disable=SC2034 # read by scripts/harden-other-model-review.sh, which sources this
OMR_RETRY_DELAY_S=${HARDEN_REVIEW_RETRY_DELAY_S:-15}
# The engine background task's kill timer. It covers two timed-out attempts,
# both probe waits and the retry wait, so the status line is always written.
# shellcheck disable=SC2034 # read by scripts/harden-other-model-review.sh, which sources this
OMR_TASK_TIMEOUT_S=2100
# A later iteration's fix this small skips the review (ADR 0476).
OMR_SMALL_FIX_LINES=60
# A reviewer reads code. It holds no Lucidos write authority.
# shellcheck disable=SC2034 # read by scripts/harden-other-model-review.sh, which sources this
OMR_SCRUBBED_ENV="LUCIDOS_AGENT_ORIGIN_TOKEN LUCIDOS_DATABASE_URL"

# Fails a readiness probe with the reason the status line quotes.
omr_unavailable() { # <why>
    # shellcheck disable=SC2034 # read by scripts/harden-other-model-review.sh, which sources this
    OMR_WHY=$1
    return 1
}

# The reviewer for a hardening backend: always the other model family.
omr_reviewer_for() { # <hardened-by>
    case "$1" in
        claude-code) echo codex ;;
        codex) echo claude ;;
        *) return 1 ;;
    esac
}

omr_label() { # <reviewer>
    case "$1" in
        codex) echo Codex ;;
        claude) echo Claude ;;
    esac
}

# One line: "run <reviewer> <base>", or a skipped status line. Exit 2 and an
# unavailable status line for an unknown backend.
omr_decide() { # <hardened-by> <docs-only 0|1> <scope> <base> <fix-lines or "">
    local reviewer label
    if ! reviewer=$(omr_reviewer_for "$1"); then
        echo "Other-model review: unavailable (unknown backend \"$1\")"
        return 2
    fi
    label=$(omr_label "$reviewer")
    if [ "$2" = 1 ]; then
        echo "$label review: skipped (docs-only)"
    elif [ "$3" = MERGE_ONLY ]; then
        echo "$label review: skipped (merge-only)"
    elif [ -n "$5" ] && [ "$5" -le "$OMR_SMALL_FIX_LINES" ]; then
        echo "$label review: skipped (a fix of $5 lines, $OMR_SMALL_FIX_LINES or fewer)"
    else
        echo "run $reviewer ${4:-main}"
    fi
}

# Sends TERM to <pid> and every process below it.
omr_kill_tree() { # <pid>
    local child
    for child in $(pgrep -P "$1" 2> /dev/null); do
        omr_kill_tree "$child"
    done
    kill -TERM "$1" 2> /dev/null
}

# Runs <cmd> and kills its process tree after <secs>. The command stays in the
# caller's process group, which `lucidos background-task stop` kills.
# Returns the command's exit status, or 124 on a timeout.
omr_with_timeout() { # <secs> <cmd>...
    local secs=$1 pid waited=0
    shift
    "$@" &
    pid=$!
    while kill -0 "$pid" 2> /dev/null; do
        if [ "$waited" -ge "$secs" ]; then
            omr_kill_tree "$pid"
            wait "$pid" 2> /dev/null
            return 124
        fi
        sleep 1
        waited=$((waited + 1))
    done
    wait "$pid"
}

# The first line naming an HTTP error status, JSON escapes undone.
omr_http_status_line() { # <file>...
    awk '{ gsub(/\\n/, "\n"); gsub(/\\"/, "\""); print }' "$@" 2> /dev/null |
        grep -E -m 1 '(^|[^0-9])[45][0-9]{2} [A-Z][A-Za-z]+' | sed 's/^[[:space:]]*//' | cut -c1-240
}

# The last non-blank line of a file, cut to fit a status line.
omr_last_line() { # <file>
    grep -v '^[[:space:]]*$' "$1" 2> /dev/null | tail -1 | cut -c1-240
}

# The status line must stay one line, and a logged error can span several.
omr_one_line() {
    tr '\r\n' '  ' | sed 's/ *$//'
}

# --- Codex, through the companion the `codex` Claude Code plugin installs ---

OMR_CODEX_GENERIC="Reviewer failed to output a response."

# The companion collapses a failure of either check into one generic error. An
# `npm install -g` of the CLI fails both for a few seconds, so ride it out.
omr_codex_ready() {
    local probe
    OMR_CODEX_COMPANION=$(find "$HOME/.claude/plugins" -name codex-companion.mjs -path '*codex*' 2> /dev/null | sort | tail -1)
    if [ -z "$OMR_CODEX_COMPANION" ]; then
        omr_unavailable "plugin not installed"
        return
    fi
    echo "CODEX_COMPANION=$OMR_CODEX_COMPANION"
    for probe in 1 2 3; do
        if codex --version > /dev/null 2>&1 && codex app-server --help > /dev/null 2>&1; then
            return 0
        fi
        if [ "$probe" != 3 ]; then
            echo "Codex CLI not answering (probe $probe of 3), an update may be rewriting it, retrying in ${OMR_PROBE_DELAY_S}s"
            sleep "$OMR_PROBE_DELAY_S"
        fi
    done
    omr_unavailable "CLI not answering after 3 probes"
}

# Never the companion's `--background` flag, which `review` parses and ignores.
omr_codex_run() { # <base>
    node "$OMR_CODEX_COMPANION" review --scope branch --base "$1" --json
}

omr_codex_show() { # <out>
    cat "$1"
}

# A reviewer that says nothing, or only the generic sentence, gave no verdict.
omr_codex_verdict() { # <exit> <out>
    local said
    said=$(omr_codex_said "$2")
    [ "$1" = 0 ] && [ -n "$said" ] && [ "$said" != "$OMR_CODEX_GENERIC" ]
}

# The JSON "stdout" field, even when empty, else the last line it printed.
omr_codex_said() { # <out>
    if grep -E -q '"stdout": *"' "$1"; then
        grep -E -o -m 1 '"stdout": *"[^"]*"' "$1" | sed -E 's/^"stdout": *"//; s/"$//'
    else
        omr_last_line "$1"
    fi
}

# The newest sampling error Codex logged at or after <epoch>, from this
# review's thread or from no thread. Another session's thread never counts.
omr_codex_logged_sampling_error() { # <epoch> <out>
    local db="" f thread
    command -v sqlite3 > /dev/null 2>&1 || return 0
    for f in "$HOME"/.codex/logs_*.sqlite; do
        [ -f "$f" ] || continue
        if [ -z "$db" ] || [ "$f" -nt "$db" ]; then db=$f; fi
    done
    [ -n "$db" ] || return 0
    thread=$(grep -E -o -m 1 '"threadId": *"[A-Za-z0-9-]+"' "$2" | sed -E 's/.*"([^"]+)"$/\1/')
    sqlite3 -readonly "file:$db?mode=ro" \
        "select substr(feedback_log_body, instr(feedback_log_body, 'sampling_error='), 240)
         from logs where ts >= $1 and feedback_log_body like '%sampling_error=%'
           and (thread_id = '$thread' or thread_id is null)
         order by thread_id is null, id desc limit 1" 2> /dev/null
}

# The companion folds an upstream failure, like a 403 for an unreachable
# model, into the generic sentence. The real cause is in the reviewer's
# stderr, its JSON payload, or Codex's own log, in that order.
omr_codex_upstream() { # <out> <err> <started>
    local found
    found=$(omr_http_status_line "$2" "$1")
    [ -n "$found" ] || found=$(omr_codex_logged_sampling_error "$3" "$1")
    [ -n "$found" ] || found=$(omr_last_line "$2")
    printf '%s\n' "${found:-none reported}" | omr_one_line
}

# --- Claude, through the `claude` CLI in print mode ---

# Every flag is defined here, once (plan decision 7). The reviewer gets no
# Bash: it reads the diff this script writes, so it cannot run a build.
OMR_CLAUDE_DIFF=.lucidos/other-model-review.diff
OMR_CLAUDE_FINDINGS_SCHEMA='{"type":"object","properties":{"findings":{"type":"array","items":{"type":"object","properties":{"file":{"type":"string"},"line":{"type":"integer"},"summary":{"type":"string"},"failure_scenario":{"type":"string"},"category":{"type":"string","enum":["correctness","compliance","cleanup"]}},"required":["file","line","summary","failure_scenario","category"]}}},"required":["findings"]}'
OMR_CLAUDE_SYSTEM_DIRS=${HARDEN_REVIEW_CLAUDE_DIRS-/opt/homebrew/bin /usr/local/bin}

# The engine's resolve_claude_binary order: the user's install, then the
# system ones, then PATH.
omr_claude_binary() {
    local dir
    for dir in "$HOME/.local/bin" "$HOME/.claude/local" $OMR_CLAUDE_SYSTEM_DIRS; do
        if [ -e "$dir/claude" ]; then
            echo "$dir/claude"
            return
        fi
    done
    command -v claude
}

omr_claude_ready() {
    local probe
    if ! OMR_CLAUDE_BIN=$(omr_claude_binary); then
        omr_unavailable "CLI not installed"
        return
    fi
    if ! command -v jq > /dev/null 2>&1; then
        omr_unavailable "jq not installed"
        return
    fi
    echo "CLAUDE=$OMR_CLAUDE_BIN"
    for probe in 1 2 3; do
        "$OMR_CLAUDE_BIN" --version > /dev/null 2>&1 && return 0
        if [ "$probe" != 3 ]; then
            echo "Claude CLI not answering (probe $probe of 3), an update may be rewriting it, retrying in ${OMR_PROBE_DELAY_S}s"
            sleep "$OMR_PROBE_DELAY_S"
        fi
    done
    omr_unavailable "CLI not answering after 3 probes"
}

# Never set or unset CLAUDE_CONFIG_DIR, and pass no --bare or
# --setting-sources: each one breaks the inherited login (plan decision 8).
omr_claude_run() { # <base>
    cd "$(git rev-parse --show-toplevel)" || return
    mkdir -p "$(dirname "$OMR_CLAUDE_DIFF")" || return
    {
        git log --format='commit %H%n%n    %s%n' "$1..HEAD" &&
            git diff "$1...HEAD"
    } > "$OMR_CLAUDE_DIFF" || return
    "$OMR_CLAUDE_BIN" -p "Review the change in $OMR_CLAUDE_DIFF, which holds the commit log and the diff of $1...HEAD in this repository. Follow .claude/skills/code-review/SKILL.md at effort high as the other-model reviewer: run its correctness angles and its verification, read any file you need, and change nothing. Return the findings that survive as structured output, with an empty list when none do." \
        --output-format json --json-schema "$OMR_CLAUDE_FINDINGS_SCHEMA" \
        --model opus --effort high \
        --tools Read,Grep,Glob --permission-mode dontAsk --permission-prompts none \
        --strict-mcp-config --no-session-persistence --max-budget-usd 5
}

omr_claude_is_result() { # <out>
    jq -e 'type == "object"' "$1" > /dev/null 2>&1
}

# The served model, the cost, then one line per finding, never a raw array.
omr_claude_show() { # <out>
    if ! omr_claude_is_result "$1"; then
        cat "$1"
        return
    fi
    jq -r '
        def one_line: tostring | gsub("[\r\n]+"; " ");
        "Claude reviewer: \((.modelUsage // {}) | keys | join(", ") | if . == "" then "model unknown" else . end), $\(.total_cost_usd // 0)",
        (if (.structured_output.findings | type) == "array" then
            if (.structured_output.findings | length) == 0 then "No findings."
            else .structured_output.findings[] | "\(.file):\(.line) [\(.category)] \(.summary): \(.failure_scenario)" | one_line
            end
        else "result: \(.result // "" | one_line)"
        end)' "$1"
}

omr_claude_verdict() { # <exit> <out>
    [ "$1" = 0 ] &&
        jq -e '.is_error == false and (.structured_output.findings | type) == "array"' "$2" > /dev/null 2>&1
}

omr_claude_said() { # <out>
    if omr_claude_is_result "$1"; then
        jq -r '.result // "" | tostring' "$1" | omr_one_line | cut -c1-240
    else
        omr_last_line "$1" | omr_one_line
    fi
}

# An HTTP status anywhere, else the result's own error flags, else stderr.
omr_claude_upstream() { # <out> <err> <started>
    local found
    found=$(omr_http_status_line "$2" "$1")
    [ -n "$found" ] || found=$(jq -r '
        if .is_error then "is_error, subtype \(.subtype // "none")"
        elif (.structured_output.findings | type) != "array" then "no structured_output, subtype \(.subtype // "none")"
        else empty
        end' "$1" 2> /dev/null)
    [ -n "$found" ] || found=$(omr_last_line "$2")
    printf '%s\n' "${found:-none reported}" | omr_one_line
}
