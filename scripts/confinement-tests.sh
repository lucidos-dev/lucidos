#!/usr/bin/env bash
#
# Run the confinement tests: the tests that apply the real agent sandbox and
# prove what it denies. Each one carries an #[ignore] naming this script, so
# the ordinary suites never meet them, and each fails when the sandbox cannot
# apply rather than skipping.
#
# A seatbelt never nests on macOS, so a coding-agent session inside the agent
# sandbox cannot run them in place. The route is picked by a real seatbelt
# trial, never by LUCIDOS_AGENT_SANDBOXED:
#   - Linux, or a macOS trial that applies: run here. Landlock nests. A
#     kernel without Landlock exits 75, since nothing was tested.
#   - macOS trial refused inside a coding-agent thread: ask the engine to run
#     this script outside the session sandbox, under the owner's
#     run-confinement-tests permission (`lucidos confinement-tests run`). With
#     --hold-secs 0, a passing run at the clean HEAD of this thread stands in
#     for the permission.
#   - macOS trial refused anywhere else: fail and say why.
#
# Usage:
#   ./scripts/confinement-tests.sh                 # pick the route by the trial
#   ./scripts/confinement-tests.sh --here          # run here, whatever the trial says
#   ./scripts/confinement-tests.sh --here --revision SHA
#                                                  # run here, failing unless the tree
#                                                  # stays at SHA with nothing uncommitted
#   ./scripts/confinement-tests.sh --hold-secs N   # on the engine route, hold the
#                                                  # owner's card N s (0: ask nobody)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Each confinement test panics with this phrase when the sandbox's own probe
# says this host cannot apply it, as on a Linux kernel without Landlock.
UNAVAILABLE="the real sandbox is unavailable"

run_tests() {
    local slot=("$ROOT/scripts/with-build-slot.sh" --label "confinement tests" --)
    "${slot[@]}" cargo test --locked -p lucidos-agent-sandbox --test enforced -- --ignored &&
        "${slot[@]}" cargo test --locked -p lucidos-engine --lib -- --ignored \
            runtime::python::sandbox_regression_tests runtime::agent_sandbox::tests:: \
            runtime::browser::session::tests::the_agent_browser_
}

# Exits 75 when the sandbox could not apply here: nothing was tested.
run_here() {
    local log status=0
    log="$(mktemp)"
    run_tests 2>&1 | tee "$log" || status=$?
    if [ "$status" -ne 0 ] && grep -q "$UNAVAILABLE" "$log"; then
        echo "[confinement] the agent sandbox cannot apply on this host, so no confinement test ran" >&2
        status=75
    fi
    rm -f "$log"
    return "$status"
}

# Whether the tree is at commit $1 with nothing uncommitted or untracked.
# The flags override any git config that would hide a change.
tree_at() {
    [ "$(git rev-parse HEAD)" = "$1" ] &&
        [ -z "$(git -c core.fsmonitor=false status --porcelain --untracked-files=all --ignore-submodules=none)" ]
}

# Runs here for a run the engine may credit to commit $1. A tree that leaves
# that commit fails the run, since the tests then built something else.
run_here_at() {
    local status=0
    if ! tree_at "$1"; then
        echo "[confinement] the tree is not at commit $1 with nothing uncommitted, so nothing ran" >&2
        return 1
    fi
    run_here || status=$?
    if [ "$status" -eq 0 ] && ! tree_at "$1"; then
        echo "[confinement] the tree left commit $1 while the tests ran, so this run proves nothing. Commit, then run again." >&2
        status=1
    fi
    return "$status"
}

# Whether this process may apply a seatbelt of its own.
seatbelt_applies() {
    sandbox-exec -p '(version 1)(allow default)' /usr/bin/true > /dev/null 2>&1
}

usage() {
    echo "usage: $0 [--here [--revision SHA] | --hold-secs N]" >&2
    exit 2
}

# Passed to the CLI on the engine route only. Running here asks nobody.
CLI_ARGS=()
case "${1:-}" in
    --here)
        case "${2:-}" in
            "") run_here ;;
            --revision)
                [ -n "${3:-}" ] || usage
                run_here_at "$3"
                ;;
            *) usage ;;
        esac
        exit
        ;;
    --hold-secs)
        [ -n "${2:-}" ] || usage
        CLI_ARGS=(--hold-secs "$2")
        ;;
    "") ;;
    *) usage ;;
esac

case "$(uname -s)" in
    Linux)
        run_here
        ;;
    Darwin)
        if seatbelt_applies; then
            run_here
        elif [ -n "${LUCIDOS_THREAD_ID:-}" ]; then
            echo "[confinement] this process cannot apply a seatbelt, so the engine runs the tests outside the session sandbox"
            exec lucidos confinement-tests run ${CLI_ARGS[@]+"${CLI_ARGS[@]}"}
        else
            echo "[confinement] this process cannot apply a seatbelt (it already runs inside one), and no Lucidos thread can run the tests outside it" >&2
            exit 1
        fi
        ;;
    *)
        echo "[confinement] the agent sandbox has no backend on $(uname -s)" >&2
        exit 1
        ;;
esac
