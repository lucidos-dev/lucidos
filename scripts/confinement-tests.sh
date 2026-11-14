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
#     kernel without Landlock routes to GitHub Linux through e2e GitHub mode.
#     An unavailable route or missing verdict exits 75, never a pass.
#   - macOS trial refused inside a coding-agent thread: ask the engine to run
#     this script outside the session sandbox, under the owner's
#     run-confinement-tests permission (`lucidos confinement-tests run`).
#   - macOS trial refused anywhere else: fail and say why.
#
# Usage:
#   ./scripts/confinement-tests.sh                 # pick the route by the trial
#   ./scripts/confinement-tests.sh --here          # run here, whatever the trial says
#   ./scripts/confinement-tests.sh --commit SHA    # pin the GitHub leg
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
            runtime::python::sandbox_regression_tests runtime::agent_sandbox::tests::
}

# Exits 75 when the sandbox could not apply here: nothing was tested.
run_here() {
    local log status=0
    SANDBOX_UNAVAILABLE=""
    log="$(mktemp)"
    run_tests 2>&1 | tee "$log" || status=$?
    if [ "$status" -ne 0 ] && grep -q "$UNAVAILABLE" "$log"; then
        echo "[confinement] the agent sandbox cannot apply on this host, so no confinement test ran" >&2
        status=75
        SANDBOX_UNAVAILABLE=1
    fi
    rm -f "$log"
    return "$status"
}

# Whether this process may apply a seatbelt of its own.
seatbelt_applies() {
    sandbox-exec -p '(version 1)(allow default)' /usr/bin/true > /dev/null 2>&1
}

usage() {
    echo "usage: $0 [--here] [--hold-secs N] [--commit SHA]" >&2
    exit 2
}

# Passed to the CLI on the engine route only. Running here asks nobody.
CLI_ARGS=()
GITHUB_ARGS=()
HERE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --here) HERE=1 ;;
        --hold-secs)
            [ $# -ge 2 ] && [ -n "$2" ] || usage
            CLI_ARGS=(--hold-secs "$2")
            shift
            ;;
        --commit)
            [ $# -ge 2 ] && [ -n "$2" ] || usage
            GITHUB_ARGS=(--commit "$2")
            shift
            ;;
        *) usage ;;
    esac
    shift
done
if [ -n "$HERE" ]; then
    run_here
    exit
fi

run_github() {
    local status=0
    echo "[confinement] trying the Linux sandbox on a GitHub runner"
    "$ROOT/scripts/e2e-github.sh" confinement ${GITHUB_ARGS[@]+"${GITHUB_ARGS[@]}"} || status=$?
    case "$status" in
        0) return 0 ;;
        75 | 76)
            echo "[confinement] GitHub produced no confinement verdict (exit $status); see the reason above" >&2
            return 75
            ;;
        *) return 1 ;;
    esac
}

case "$(uname -s)" in
    Linux)
        status=0
        run_here || status=$?
        if [ -n "$SANDBOX_UNAVAILABLE" ]; then
            run_github
        else
            exit "$status"
        fi
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
