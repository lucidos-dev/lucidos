#!/bin/bash
# The reviewer adapters and shared helpers behind scripts/harden-other-model-review.sh.
#
# Each reviewer <r> is a set of functions, called by the entry script:
#
#   omr_<r>_ready              probe the reviewer; on failure set OMR_WHY, return 1
#   omr_<r>_run <base> <head>  review <base>...<head>; stdout and stderr are files
#   omr_<r>_show <out>         print the attempt's review for the .out record
#   omr_<r>_verdict <exit> <out>  return 0 when the attempt proves a read of the
#                              patch; otherwise OMR_REJECTED may name why not
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

# --- The range under review, and the verdict both reviewers return ---

# The diff each reviewer reads, and the file list the proof check expects from
# it. One spelling for all: Git's default quoting escapes a non-ASCII path, and
# a rename shows two paths in a diff but one in a name-only list.
OMR_DIFF=(git -c core.quotePath=false diff --no-renames)
OMR_NAME_ONLY=("${OMR_DIFF[@]}" --name-only)

# Resolves the range once, so every attempt and the proof check see the same
# commits. Sets OMR_BASE and OMR_HEAD, and writes the diff's file list to
# $OMR_WORK/changed-files.
omr_freeze_range() { # <base>
    if ! OMR_BASE=$(git rev-parse --verify --quiet "$1^{commit}"); then
        omr_unavailable "base \"$1\" is not a commit"
        return
    fi
    if ! OMR_HEAD=$(git rev-parse --verify --quiet "HEAD^{commit}"); then
        omr_unavailable "HEAD is not a commit"
        return
    fi
    "${OMR_NAME_ONLY[@]}" "$OMR_BASE...$OMR_HEAD" > "$OMR_WORK/changed-files" ||
        omr_unavailable "git diff $OMR_BASE...$OMR_HEAD failed"
}

# One schema for both reviewers. `status` and `changed_files` are the proof
# of read: a reviewer that could not open the diff cannot name its files.
OMR_FINDINGS_SCHEMA='{"type":"object","additionalProperties":false,"properties":{"status":{"type":"string","enum":["reviewed","unable"]},"summary":{"type":"string"},"changed_files":{"type":"array","items":{"type":"string"}},"findings":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"file":{"type":"string"},"line":{"type":"integer"},"summary":{"type":"string"},"failure_scenario":{"type":"string"},"category":{"type":"string","enum":["correctness","compliance","cleanup"]}},"required":["file","line","summary","failure_scenario","category"]}}},"required":["status","summary","changed_files","findings"]}'

# What every reviewer is told about the proof, so it means the same for both.
omr_proof_ask() { # <base> <head>
    echo "Set status to reviewed only after you have read the whole diff. Otherwise set it to unable, and say why in summary. List in changed_files every path the diff changes, exactly as \`${OMR_NAME_ONLY[*]} $1...$2\` prints it."
}

# Returns 0 only when <verdict> proves a read of the frozen range. Otherwise
# sets OMR_REJECTED to the reason the status line quotes.
omr_proves_read() { # <verdict JSON, or empty>
    local mismatch
    if ! printf '%s' "$1" | jq -e 'type == "object" and (.findings | type) == "array" and (.changed_files | type) == "array"' > /dev/null 2>&1; then
        OMR_REJECTED="no structured verdict"
        return 1
    fi
    if ! printf '%s' "$1" | jq -e '.status == "reviewed"' > /dev/null 2>&1; then
        OMR_REJECTED="the reviewer said it could not review"
        return 1
    fi
    if ! mismatch=$(printf '%s' "$1" | jq -r --rawfile want "$OMR_WORK/changed-files" '
        ($want | split("\n") | map(select(. != "")) | unique) as $w
        | (.changed_files | map(tostring | ltrimstr("./")) | unique) as $g
        | (($w - $g | map("missing " + .)) + ($g - $w | map("unexpected " + .)))
        | .[:4] | join(", ")' 2> /dev/null); then
        OMR_REJECTED="could not compare its changed files with the diff"
        return 1
    fi
    if [ -n "$mismatch" ]; then
        OMR_REJECTED="its changed files do not match the diff: $mismatch"
        return 1
    fi
}

# The summary, then one line per finding, never a raw array.
omr_show_verdict() { # <verdict JSON>
    printf '%s' "$1" | jq -r '
        def one_line: tostring | gsub("[\r\n]+"; " ");
        "summary: \(.summary // "" | one_line)",
        "status: \(.status // "none" | one_line), changed files named: \(.changed_files // [] | length)",
        (if (.findings | type) != "array" then empty
         elif (.findings | length) == 0 then "No findings."
         else .findings[] | "\(.file):\(.line) [\(.category)] \(.summary): \(.failure_scenario)" | one_line
         end)'
}

# --- Codex, through `codex exec` ---

# Codex's own sandbox mode, from Codex's own answer. A seatbelt does not nest
# (ADR 0645), so inside the Lucidos agent sandbox Codex runs with its own off,
# as the engine's Codex sessions do. Outside one, a sandbox that cannot apply
# fails closed. Sets OMR_CODEX_SANDBOX.
omr_codex_pick_sandbox() {
    local refused
    if refused=$(codex sandbox -- true 2>&1); then
        OMR_CODEX_SANDBOX=read-only
        echo "CODEX_SANDBOX=$OMR_CODEX_SANDBOX"
    elif [ "${LUCIDOS_AGENT_SANDBOXED:-}" = 1 ]; then
        OMR_CODEX_SANDBOX=danger-full-access
        echo "CODEX_SANDBOX=$OMR_CODEX_SANDBOX (Codex's own sandbox cannot nest in the Lucidos agent sandbox, which bounds this review instead, ADR 0645)"
    else
        omr_unavailable "Codex's sandbox cannot run here and no Lucidos sandbox wraps this review: $(printf '%s\n' "$refused" | grep -v '^[[:space:]]*$' | tail -1 | cut -c1-200)"
    fi
}

omr_codex_ready() {
    local probe
    if ! command -v codex > /dev/null 2>&1; then
        omr_unavailable "CLI not installed"
        return
    fi
    if ! command -v jq > /dev/null 2>&1; then
        omr_unavailable "jq not installed"
        return
    fi
    for probe in 1 2 3; do
        if codex --version > /dev/null 2>&1 && codex exec --help > /dev/null 2>&1; then
            omr_codex_pick_sandbox
            return
        fi
        if [ "$probe" != 3 ]; then
            echo "Codex CLI not answering (probe $probe of 3), an update may be rewriting it, retrying in ${OMR_PROBE_DELAY_S}s"
            sleep "$OMR_PROBE_DELAY_S"
        fi
    done
    omr_unavailable "CLI not answering after 3 probes"
}

# Each attempt reviews a fresh snapshot of the frozen head, so a write the
# reviewer makes never reaches the branch under review. Approvals have no
# channel here, so none are asked.
omr_codex_run() { # <base> <head>
    local tree="$OMR_WORK/codex-tree" schema="$OMR_WORK/findings-schema.json"
    rm -rf "$tree" || return
    git clone --quiet --shared --no-checkout "$(git rev-parse --show-toplevel)" "$tree" || return
    git -C "$tree" checkout --quiet --detach "$2" || return
    printf '%s\n' "$OMR_FINDINGS_SCHEMA" > "$schema" || return
    printf '%s\n' "Review the change $1...$2 in this repository, a snapshot of the branch under review. Read its commit log (git log $1..$2) and its diff (${OMR_DIFF[*]} $1...$2). Follow .claude/skills/code-review/SKILL.md at effort high as the other-model reviewer: run its correctness angles and its verification, read any file you need, and change nothing. Return the findings that survive, with an empty list when none do. $(omr_proof_ask "$1" "$2")" |
        codex exec -C "$tree" --sandbox "$OMR_CODEX_SANDBOX" -c 'approval_policy="never"' \
            --ephemeral --json --output-schema "$schema" -
}

# Every JSON object in the event stream, one per line. Other lines are dropped.
omr_codex_events() { # <out>
    jq -R -c 'fromjson? | select(type == "object")' "$1" 2> /dev/null
}

# The reviewer's last message, as it wrote it.
omr_codex_final_message() { # <out>
    omr_codex_events "$1" |
        jq -c 'select(.type == "item.completed" and (.item | type) == "object" and .item.type == "agent_message") | .item.text' |
        tail -1 | jq -r '.'
}

# The last message as a JSON object, or nothing when it is not one.
omr_codex_verdict_json() { # <out>
    omr_codex_final_message "$1" | jq -c 'select(type == "object")' 2> /dev/null
}

# What the reviewer ran and what failed, then its verdict or its last words.
omr_codex_show() { # <out>
    local verdict
    if [ -z "$(omr_codex_events "$1" | head -1)" ]; then
        cat "$1"
        return
    fi
    omr_codex_events "$1" | jq -r '
        def one_line: tostring | gsub("[\r\n]+"; " ");
        if .type == "item.completed" and (.item | type) == "object" and .item.type == "command_execution" then
            "ran: \(.item.command | one_line | .[:200]) (exit \(.item.exit_code // "none"))"
        elif .type == "turn.failed" then "turn failed: \(.error.message // "" | one_line)"
        elif .type == "error" then "error: \(.message // "" | one_line)"
        else empty
        end'
    verdict=$(omr_codex_verdict_json "$1")
    if [ -n "$verdict" ]; then
        omr_show_verdict "$verdict"
    else
        echo "last message: $(omr_codex_final_message "$1" | omr_one_line | cut -c1-500)"
    fi
}

omr_codex_verdict() { # <exit> <out>
    [ "$1" = 0 ] && omr_proves_read "$(omr_codex_verdict_json "$2")"
}

# The verdict's summary, else the last message, else the last line printed.
omr_codex_said() { # <out>
    local verdict
    verdict=$(omr_codex_verdict_json "$1")
    if [ -n "$verdict" ]; then
        printf '%s' "$verdict" | jq -r '.summary // "" | tostring'
    elif [ -n "$(omr_codex_events "$1" | head -1)" ]; then
        omr_codex_final_message "$1"
    else
        omr_last_line "$1"
    fi | omr_one_line | cut -c1-240
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
    thread=$(grep -E -o -m 1 '"thread_id": *"[A-Za-z0-9-]+"' "$2" | sed -E 's/.*"([^"]+)"$/\1/')
    sqlite3 -readonly "file:$db?mode=ro" \
        "select substr(feedback_log_body, instr(feedback_log_body, 'sampling_error='), 240)
         from logs where ts >= $1 and feedback_log_body like '%sampling_error=%'
           and (thread_id = '$thread' or thread_id is null)
         order by thread_id is null, id desc limit 1" 2> /dev/null
}

# The real cause, in order: an HTTP status anywhere, the turn's own error, a
# command that failed (a sandbox refusal lands here), Codex's log, stderr.
omr_codex_upstream() { # <out> <err> <started>
    local found
    found=$(omr_http_status_line "$2" "$1")
    [ -n "$found" ] || found=$(omr_codex_events "$1" | jq -r '
        if .type == "turn.failed" then .error.message // empty
        elif .type == "error" then .message // empty
        else empty
        end' | tail -1)
    [ -n "$found" ] || found=$(omr_codex_events "$1" | jq -r '
        select(.type == "item.completed" and (.item | type) == "object" and .item.type == "command_execution"
            and (.item.exit_code // 0) != 0)
        | (.item.aggregated_output // "" | split("\n") | map(select(test("\\S"))) | last) as $said
        | select($said != null)
        | "a command failed (exit \(.item.exit_code)): \($said)"' | head -1)
    [ -n "$found" ] || found=$(omr_codex_logged_sampling_error "$3" "$1")
    [ -n "$found" ] || found=$(omr_last_line "$2")
    printf '%s\n' "${found:-none reported}" | omr_one_line | cut -c1-240
}

# --- Claude, through the `claude` CLI in print mode ---

# Every flag is defined here, once (plan decision 7). The reviewer gets no
# Bash: it reads the diff this script writes, so it cannot run a build, and
# no seatbelt of its own ever nests in the Lucidos sandbox.
OMR_CLAUDE_DIFF=.lucidos/other-model-review.diff
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
omr_claude_run() { # <base> <head>
    cd "$(git rev-parse --show-toplevel)" || return
    mkdir -p "$(dirname "$OMR_CLAUDE_DIFF")" || return
    {
        git log --format='commit %H%n%n    %s%n' "$1..$2" &&
            "${OMR_DIFF[@]}" "$1...$2"
    } > "$OMR_CLAUDE_DIFF" || return
    "$OMR_CLAUDE_BIN" -p "Review the change in $OMR_CLAUDE_DIFF, which holds the commit log and the diff of $1...$2 in this repository. Follow .claude/skills/code-review/SKILL.md at effort high as the other-model reviewer: run its correctness angles and its verification, read any file you need, and change nothing. Return the findings that survive as structured output, with an empty list when none do. $(omr_proof_ask "$1" "$2")" \
        --output-format json --json-schema "$OMR_FINDINGS_SCHEMA" \
        --model opus --effort high \
        --tools Read,Grep,Glob --permission-mode dontAsk --permission-prompts none \
        --strict-mcp-config --no-session-persistence --max-budget-usd 5
}

omr_claude_is_result() { # <out>
    jq -e 'type == "object"' "$1" > /dev/null 2>&1
}

# The structured output of a result that is not an error, or nothing.
omr_claude_verdict_json() { # <out>
    jq -c 'select(type == "object" and .is_error == false) | .structured_output | select(type == "object")' "$1" 2> /dev/null
}

# The served model and the cost, then the verdict, never a raw array.
omr_claude_show() { # <out>
    local verdict
    if ! omr_claude_is_result "$1"; then
        cat "$1"
        return
    fi
    jq -r '"Claude reviewer: \((.modelUsage // {}) | keys | join(", ") | if . == "" then "model unknown" else . end), $\(.total_cost_usd // 0)"' "$1"
    verdict=$(omr_claude_verdict_json "$1")
    if [ -n "$verdict" ]; then
        omr_show_verdict "$verdict"
    else
        jq -r '"result: \(.result // "" | tostring | gsub("[\r\n]+"; " "))"' "$1"
    fi
}

omr_claude_verdict() { # <exit> <out>
    [ "$1" = 0 ] && omr_proves_read "$(omr_claude_verdict_json "$2")"
}

# The verdict's summary, else the result text, else the last line printed.
omr_claude_said() { # <out>
    if omr_claude_is_result "$1"; then
        jq -r '(.structured_output.summary? // "" | tostring) as $s
            | if $s != "" then $s else .result // "" | tostring end' "$1"
    else
        omr_last_line "$1"
    fi | omr_one_line | cut -c1-240
}

# An HTTP status anywhere, else the result's own error flags, else stderr.
omr_claude_upstream() { # <out> <err> <started>
    local found
    found=$(omr_http_status_line "$2" "$1")
    [ -n "$found" ] || found=$(jq -r '
        if .is_error then "is_error, subtype \(.subtype // "none")"
        elif (.structured_output | type) != "object" then "no structured_output, subtype \(.subtype // "none")"
        else empty
        end' "$1" 2> /dev/null)
    [ -n "$found" ] || found=$(omr_last_line "$2")
    printf '%s\n' "${found:-none reported}" | omr_one_line
}
