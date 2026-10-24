#!/bin/bash
# The other-model review `/harden` launches in Phase 1 and joins in Phase 3.
# The reviewer is always the model family other than the hardening backend:
# Codex reviews a Claude Code run, Claude reviews a Codex run (ADR 0558).
#
#   ./scripts/harden-other-model-review.sh decide --hardened-by <claude-code|codex>
#       [--docs-only] [--scope FULL|INCREMENTAL|MERGE_ONLY]
#       [--other-model-base <sha>] [--fix-lines <n>]
#   ./scripts/harden-other-model-review.sh task-timeout
#   ./scripts/harden-other-model-review.sh --hardened-by <backend> [--base <sha>]
#
# `decide` prints "run <reviewer> <base>", or a skipped status line. The
# review itself covers <base>...HEAD (default main). It prints each attempt's
# review, then ONE status line, always the last line. <R> is Codex or Claude:
#
#   <R> review: verdict returned (attempt N of 2)                          exit 0
#   <R> review: NO VERDICT after N attempts (<cause>, reviewer said
#     "...", upstream: <status or stderr line>)                            exit 1
#   <R> review: unavailable (<why>)                                        exit 2
#   <R> review: skipped (<why>)                                 `decide`, exit 0
#
# An attempt with no verdict is retried once. A timed-out attempt is not.
# `task-timeout` prints the engine background task's kill timer.
#
# Knobs, for the tests: HARDEN_REVIEW_PROBE_DELAY_S (30),
# HARDEN_REVIEW_RETRY_DELAY_S (15), HARDEN_REVIEW_ATTEMPT_TIMEOUT_S (900),
# HARDEN_REVIEW_CLAUDE_DIRS (the system dirs searched for `claude`).
# Tested by scripts/lib/harden_other_model_review_test.sh.
set -u

# shellcheck source=scripts/lib/other_model_review.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/other_model_review.sh"

usage() {
    sed -n '6,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 64
}

cmd=review
case "${1:-}" in
    decide | task-timeout)
        cmd=$1
        shift
        ;;
esac
backend="" base=main docs_only=0 scope=FULL other_base="" fix_lines=""
while [ $# -gt 0 ]; do
    case "$1" in
        --hardened-by) backend=${2:-} && shift ;;
        --base) base=${2:-} && shift ;;
        --docs-only) docs_only=1 ;;
        --scope) scope=${2:-} && shift ;;
        --other-model-base) other_base=${2:-} && shift ;;
        --fix-lines) fix_lines=${2:-} && shift ;;
        *) usage ;;
    esac
    shift
done

case "$cmd" in
    task-timeout)
        echo "$OMR_TASK_TIMEOUT_S"
        exit 0
        ;;
    decide)
        omr_decide "$backend" "$docs_only" "$scope" "$other_base" "$fix_lines"
        exit
        ;;
esac

if ! reviewer=$(omr_reviewer_for "$backend"); then
    echo "Other-model review: unavailable (unknown backend \"$backend\")"
    exit 2
fi
label=$(omr_label "$reviewer")
# shellcheck disable=SC2086 # a list of names, split on purpose
unset $OMR_SCRUBBED_ENV

WORK="$(mktemp -d)" || exit 2
trap 'rm -rf "$WORK"' EXIT

OMR_WHY=""
if ! "omr_${reviewer}_ready"; then
    echo "$label review: unavailable ($OMR_WHY)"
    exit 2
fi

attempt=1
while :; do
    out="$WORK/out.$attempt" err="$WORK/err.$attempt" started=$(date +%s)
    omr_with_timeout "$OMR_ATTEMPT_TIMEOUT_S" "omr_${reviewer}_run" "$base" > "$out" 2> "$err"
    status=$?
    echo "--- attempt $attempt of $OMR_ATTEMPTS: exit $status ---"
    "omr_${reviewer}_show" "$out"
    if [ -s "$err" ]; then
        echo "--- attempt $attempt stderr ---"
        cat "$err"
    fi
    if [ "$status" != 124 ] && "omr_${reviewer}_verdict" "$status" "$out"; then
        echo "$label review: verdict returned (attempt $attempt of $OMR_ATTEMPTS)"
        exit 0
    fi
    if [ "$status" = 124 ] || [ "$attempt" = "$OMR_ATTEMPTS" ]; then
        cause="exit $status"
        [ "$status" = 124 ] && cause="timed out after ${OMR_ATTEMPT_TIMEOUT_S}s"
        noun=attempts
        [ "$attempt" = 1 ] && noun=attempt
        said=$("omr_${reviewer}_said" "$out")
        echo "$label review: NO VERDICT after $attempt $noun ($cause, reviewer said \"${said:-nothing}\", upstream: $("omr_${reviewer}_upstream" "$out" "$err" "$started"))"
        exit 1
    fi
    echo "$label reviewer produced no verdict, retrying in ${OMR_RETRY_DELAY_S}s"
    sleep "$OMR_RETRY_DELAY_S"
    attempt=$((attempt + 1))
done
