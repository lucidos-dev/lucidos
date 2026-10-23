#!/usr/bin/env bash
# main-check.sh: run the main check on one commit of main (ADR 0464).
#
#   ./scripts/main-check.sh <sha>
#   ./scripts/main-check.sh <sha> --attach <run-url> <branch>
#
# The engine's one entry point, run after a landing moves main. It runs the
# engine suite and every e2e suite on GitHub against a stripped copy of
# <sha>, through `e2e-github.sh main-check`. Nothing runs on this host and
# nothing falls back to it. --attach picks up the run an engine left going
# when it restarted, and pushes nothing.
#
# Stdout carries only the protocol the engine reads (scripts/lib/main_check.sh):
#   main-check run <url> <branch>  once the workflow run is found
#   main-check verdict <json>      always the last line: verdict, run URL,
#                                  summary, failure tail and log path
# Exit: 0 pass, 1 fail, 76 no verdict. The driver's own output goes to a log
# under test-results/main-check/.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
# shellcheck source=scripts/lib/main_check.sh
source "$SCRIPT_DIR/lib/main_check.sh"

# The driver and the log directory are seams for scripts/lib/main_check_test.sh,
# which never reaches GitHub.
driver="${MAIN_CHECK_E2E_DRIVER:-$SCRIPT_DIR/e2e-github.sh}"
log_dir="${MAIN_CHECK_LOG_DIR:-$PROJECT_DIR/test-results/main-check}"

sha="${1:-}"
attach_args=()
usage_error=""
case "$#:${2:-}" in
    0: | 1:) ;;
    4:--attach) attach_args=(--attach "$3" --branch "$4") ;;
    *) usage_error=1 ;;
esac
mkdir -p "$log_dir"
log="$log_dir/$(date -u +%Y%m%d-%H%M%S)-${sha:0:12}.log"
: >"$log"

# Engine shutdown signals the whole group: SIGUSR2 to detach, SIGTERM to
# cancel. A trap, unlike an ignore, leaves the driver free to act on it, and
# makes this script wait for the pipeline to finish before it reports. The
# engine waits for that exit.
trap ':' TERM USR2

if [ -z "$sha" ] || [ -n "$usage_error" ]; then
    echo "ERROR: usage: $0 <sha> [--attach <run-url> <branch>]" >>"$log"
    rc="$E2E_GITHUB_NO_VERDICT_EXIT"
else
    # Under pipefail this is the driver's exit, since the follow always ends 0.
    # PIPESTATUS would not do: the TERM trap, run after the pipeline, resets it.
    "$driver" main-check --commit "$sha" ${attach_args[@]+"${attach_args[@]}"} 2>&1 | main_check_follow "$log"
    rc=$?
fi

verdict="$(main_check_verdict "$rc")"
main_check_verdict_line "$verdict" "$rc" "$log"
exit "$(main_check_exit "$verdict")"
