#!/bin/bash
# Run the Codex review that `/harden` launches in Phase 1 and joins in Phase 3.
#
#   ./scripts/harden-codex-review.sh > .lucidos/codex-review.out 2>&1
#
# Reviews main...HEAD, or CODEX_BASE...HEAD when CODEX_BASE is set. Prints each
# attempt's output, then ONE status line, which is always the last line:
#
#   Codex review: verdict returned (attempt N of 2)                       exit 0
#   Codex review: NO VERDICT after 2 attempts (exit S, reviewer said
#     "...", upstream: <status or stderr line>)                           exit 1
#   Codex review: unavailable (<why>)                                     exit 2
#
# A reviewer that exits non-zero, says nothing, or says only "Reviewer failed
# to output a response." produced no verdict. This script retries it once.
# The companion folds an upstream failure, like a 403 for an unreachable model,
# into that generic sentence. So the status line quotes the real cause from the
# reviewer's stderr, its JSON payload, or Codex's own log, in that order.
#
# Knobs, for the tests: HARDEN_CODEX_PROBE_DELAY_S (30), HARDEN_CODEX_RETRY_DELAY_S (15).
# Tested by scripts/lib/harden_codex_review_test.sh.
set -u

ATTEMPTS=2
GENERIC="Reviewer failed to output a response."
PROBE_DELAY=${HARDEN_CODEX_PROBE_DELAY_S:-30}
RETRY_DELAY=${HARDEN_CODEX_RETRY_DELAY_S:-15}

WORK="$(mktemp -d)" || exit 2
trap 'rm -rf "$WORK"' EXIT

# The first line naming an HTTP error status, JSON escapes undone.
http_status_line() { # <file>...
    awk '{ gsub(/\\n/, "\n"); gsub(/\\"/, "\""); print }' "$@" 2> /dev/null |
        grep -E -m 1 '(^|[^0-9])[45][0-9]{2} [A-Z][A-Za-z]+' | sed 's/^[[:space:]]*//' | cut -c1-240
}

# The newest sampling error Codex logged at or after <epoch>, from this
# review's thread or from no thread. Another session's thread never counts.
logged_sampling_error() { # <epoch> <stdout-file>
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

upstream_detail() { # <stdout-file> <stderr-file> <attempt-start-epoch>
    local found
    found=$(http_status_line "$2" "$1")
    [ -n "$found" ] || found=$(logged_sampling_error "$3" "$1")
    [ -n "$found" ] || found=$(grep -v '^[[:space:]]*$' "$2" 2> /dev/null | tail -1 | cut -c1-240)
    # The status line must stay one line, and a logged error can span several.
    printf '%s\n' "${found:-none reported}" | tr '\r\n' '  ' | sed 's/ *$//'
}

# What the reviewer itself said: the JSON "stdout" field, even when empty,
# else the last line it printed.
reviewer_said() { # <stdout-file>
    if grep -E -q '"stdout": *"' "$1"; then
        grep -E -o -m 1 '"stdout": *"[^"]*"' "$1" | sed -E 's/^"stdout": *"//; s/"$//'
    else
        grep -v '^[[:space:]]*$' "$1" | tail -1 | cut -c1-240
    fi
}

COMPANION=$(find "$HOME/.claude/plugins" -name codex-companion.mjs -path '*codex*' 2> /dev/null | sort | tail -1)
if [ -z "$COMPANION" ]; then
    echo "Codex review: unavailable (plugin not installed)"
    exit 2
fi
echo "CODEX_COMPANION=$COMPANION"

# The companion collapses a failure of either check into one generic error. An
# `npm install -g` of the CLI fails both for a few seconds, so ride it out.
ready=""
for probe in 1 2 3; do
    if codex --version > /dev/null 2>&1 && codex app-server --help > /dev/null 2>&1; then
        ready=1
        break
    fi
    if [ "$probe" != 3 ]; then
        echo "Codex CLI not answering (probe $probe of 3), an update may be rewriting it, retrying in ${PROBE_DELAY}s"
        sleep "$PROBE_DELAY"
    fi
done
if [ -z "$ready" ]; then
    echo "Codex review: unavailable (CLI not answering after 3 probes)"
    exit 2
fi

attempt=1
while :; do
    out="$WORK/out.$attempt" err="$WORK/err.$attempt" started=$(date +%s)
    node "$COMPANION" review --scope branch --base "${CODEX_BASE:-main}" --json > "$out" 2> "$err"
    status=$?
    echo "--- attempt $attempt of $ATTEMPTS: exit $status ---"
    cat "$out"
    if [ -s "$err" ]; then
        echo "--- attempt $attempt stderr ---"
        cat "$err"
    fi
    said=$(reviewer_said "$out")
    if [ "$status" = 0 ] && [ -n "$said" ] && [ "$said" != "$GENERIC" ]; then
        echo "Codex review: verdict returned (attempt $attempt of $ATTEMPTS)"
        exit 0
    fi
    if [ "$attempt" = "$ATTEMPTS" ]; then
        echo "Codex review: NO VERDICT after $ATTEMPTS attempts (exit $status, reviewer said \"${said:-nothing}\", upstream: $(upstream_detail "$out" "$err" "$started"))"
        exit 1
    fi
    echo "Codex reviewer produced no verdict, retrying in ${RETRY_DELAY}s"
    sleep "$RETRY_DELAY"
    attempt=$((attempt + 1))
done
